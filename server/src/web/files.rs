//! Browser <-> server file plumbing: uploads (replacing the native file /
//! folder picker), asset serving (replacing convertFileSrc / asset://),
//! downloads (replacing the native save dialog) and a server-side folder
//! browser used by the "choose folder" dialog.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::extract::{Multipart, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;

use super::paths::resolve_inside;
use super::AppState;

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

/// Turn a browser-provided relative name (file name or webkitRelativePath)
/// into safe path components.
fn sanitize_rel(rel: &str) -> Option<PathBuf> {
    let rel = rel.replace('\\', "/");
    let mut out = PathBuf::new();
    for c in Path::new(&rel).components() {
        match c {
            Component::Normal(s) => {
                let s = s.to_string_lossy();
                let cleaned: String = s
                    .chars()
                    .filter(|c| !c.is_control() && *c != ':')
                    .collect();
                if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
                    continue;
                }
                out.push(cleaned);
            }
            _ => continue,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

#[derive(Deserialize)]
pub struct UploadQuery {
    /// Optional destination directory (must be inside DATA_DIR). Defaults
    /// to a fresh DATA_DIR/uploads/<stamp>/ inbox.
    dir: Option<String>,
}

/// multipart: any number of (`relpath` text field, `file` file field)
/// pairs; `relpath` is optional and defaults to the file name.
pub async fn handle_upload(
    State(st): State<AppState>,
    Query(q): Query<UploadQuery>,
    mut mp: Multipart,
) -> Response {
    let root = &st.cfg.data_dir;
    let dest = match q.dir.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => match resolve_inside(root, root, d) {
            Ok(p) => p,
            Err(e) => return err(StatusCode::FORBIDDEN, e),
        },
        None => {
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
            let suffix: String = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
            st.cfg.uploads_dir.join(format!("{stamp}-{suffix}"))
        }
    };
    if let Err(e) = tokio::fs::create_dir_all(&dest).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("mkdir failed: {e}"));
    }

    let mut pending_rel: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    let mut top_dirs: Vec<String> = Vec::new();
    loop {
        let field = match mp.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return err(StatusCode::BAD_REQUEST, format!("upload failed: {e}")),
        };
        let name = field.name().unwrap_or("").to_string();
        if name == "relpath" {
            pending_rel = field.text().await.ok();
            continue;
        }
        if name != "file" {
            continue;
        }
        let rel = pending_rel
            .take()
            .filter(|s| !s.is_empty())
            .or_else(|| field.file_name().map(|s| s.to_string()))
            .unwrap_or_else(|| "upload.bin".into());
        let Some(rel_path) = sanitize_rel(&rel) else {
            return err(StatusCode::BAD_REQUEST, format!("invalid file name {rel}"));
        };
        if rel_path.components().count() > 1 {
            if let Some(Component::Normal(first)) = rel_path.components().next() {
                let first = first.to_string_lossy().to_string();
                if !top_dirs.contains(&first) {
                    top_dirs.push(first);
                }
            }
        }
        let target = dest.join(&rel_path);
        if let Some(parent) = target.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return err(StatusCode::INTERNAL_SERVER_ERROR, format!("mkdir failed: {e}"));
            }
        }
        let mut out = match tokio::fs::File::create(&target).await {
            Ok(f) => f,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("create failed: {e}")),
        };
        let mut field = field;
        loop {
            match field.chunk().await {
                Ok(Some(bytes)) => {
                    if let Err(e) = out.write_all(&bytes).await {
                        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("write failed: {e}"));
                    }
                }
                Ok(None) => break,
                Err(e) => return err(StatusCode::BAD_REQUEST, format!("upload interrupted: {e}")),
            }
        }
        let _ = out.flush().await;
        files.push(target.to_string_lossy().to_string());
    }

    let dirs: Vec<String> = top_dirs
        .iter()
        .map(|d| dest.join(d).to_string_lossy().to_string())
        .collect();
    Json(json!({
        "root": dest.to_string_lossy(),
        "paths": files,
        "dirs": dirs,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct PathQuery {
    path: String,
    name: Option<String>,
}

async fn serve_file(st: &AppState, q: &PathQuery, req: Request, attachment: bool) -> Response {
    let root = &st.cfg.data_dir;
    let path = match resolve_inside(root, root, &q.path) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::FORBIDDEN, e),
    };
    if !path.is_file() {
        return err(StatusCode::NOT_FOUND, "file not found");
    }
    // Never hand out the login hash / session table, even to a signed-in
    // browser (an XSS or a shared link would otherwise leak them).
    if path.parent() == Some(root.as_path())
        && matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("auth.json") | Some("sessions.json")
        )
    {
        return err(StatusCode::FORBIDDEN, "forbidden");
    }
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let svc = tower_http::services::ServeFile::new_with_mime(&path, &mime);
    let mut res = match svc.oneshot(req).await {
        Ok(r) => r.map(Body::new),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    // Never let an uploaded HTML/SVG run script in our origin.
    let essence = mime.essence_str();
    if essence.contains("html") || essence.contains("svg") || essence.contains("xml") || essence.contains("javascript") {
      h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; img-src 'self' data:; media-src 'self'; style-src 'unsafe-inline'; sandbox"),
      );
    }
    if attachment {
        let name = q
            .name
            .clone()
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "download".into());
        let encoded = percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC);
        if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{encoded}")) {
            h.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    res
}

pub async fn handle_asset(State(st): State<AppState>, Query(q): Query<PathQuery>, req: Request) -> Response {
    serve_file(&st, &q, req, false).await
}

pub async fn handle_download(State(st): State<AppState>, Query(q): Query<PathQuery>, req: Request) -> Response {
    serve_file(&st, &q, req, true).await
}

#[derive(Deserialize)]
pub struct ListQuery {
    path: Option<String>,
    #[serde(default)]
    files: bool,
}

pub async fn handle_fs_list(State(st): State<AppState>, Query(q): Query<ListQuery>) -> Response {
    let root = &st.cfg.data_dir;
    let start = q
        .path
        .clone()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| st.cfg.projects_dir.to_string_lossy().to_string());
    let dir = match resolve_inside(root, root, &start) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::FORBIDDEN, e),
    };
    let mut entries: Vec<Value> = Vec::new();
    match std::fs::read_dir(&dir) {
        Ok(rd) => {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if !is_dir && !q.files {
                    continue;
                }
                let p = e.path();
                let is_project = is_dir && (p.join("schema.md").is_file() && p.join("wiki").is_dir());
                entries.push(json!({
                    "name": name,
                    "path": p.to_string_lossy(),
                    "isDir": is_dir,
                    "isProject": is_project,
                }));
            }
        }
        Err(e) => return err(StatusCode::NOT_FOUND, format!("cannot read {}: {e}", dir.display())),
    }
    entries.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
    });
    let parent = if dir == *root {
        Value::Null
    } else {
        dir.parent()
            .map(|p| Value::String(p.to_string_lossy().to_string()))
            .unwrap_or(Value::Null)
    };
    Json(json!({
        "path": dir.to_string_lossy(),
        "parent": parent,
        "root": root.to_string_lossy(),
        "entries": entries,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct MkdirBody {
    path: String,
}

pub async fn handle_fs_mkdir(State(st): State<AppState>, Json(b): Json<MkdirBody>) -> Response {
    let root = &st.cfg.data_dir;
    match resolve_inside(root, root, &b.path) {
        Ok(p) => match std::fs::create_dir_all(&p) {
            Ok(()) => Json(json!({ "path": p.to_string_lossy() })).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        },
        Err(e) => err(StatusCode::FORBIDDEN, e),
    }
}

/// Imported sources are copied into the project and exports are handed to
/// the browser as downloads, so both folders are only staging areas:
/// delete entries older than 24 h, hourly (keeps the volume from filling).
pub fn spawn_upload_janitor(dirs: Vec<PathBuf>) {
    tokio::spawn(async move {
        loop {
            let cutoff = SystemTime::now() - Duration::from_secs(24 * 3600);
            for dir in &dirs {
                let Ok(rd) = std::fs::read_dir(dir) else { continue };
                for e in rd.flatten() {
                    let old = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .map(|t| t < cutoff)
                        .unwrap_or(false);
                    if !old {
                        continue;
                    }
                    let p = e.path();
                    let _ = if p.is_dir() {
                        std::fs::remove_dir_all(&p)
                    } else {
                        std::fs::remove_file(&p)
                    };
                }
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}
