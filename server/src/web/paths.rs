//! Filesystem sandbox: every path that reaches a command, the asset
//! endpoint or the upload endpoint must resolve inside DATA_DIR.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// Lexically normalise `p` (relative paths are taken relative to `base`),
/// then resolve symlinks of the longest existing prefix, and require the
/// result to live inside `root` (which must already be canonical).
pub fn resolve_inside(root: &Path, base: &Path, p: &str) -> Result<PathBuf, String> {
    if p.contains('\0') {
        return Err("invalid path".into());
    }
    let raw = Path::new(p);
    let joined = if raw.is_absolute() { raw.to_path_buf() } else { base.join(raw) };

    let mut norm = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => norm.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                norm.pop();
            }
            Component::Normal(s) => norm.push(s),
        }
    }

    // Canonicalise the deepest existing ancestor so symlinks cannot escape.
    let mut existing = norm.clone();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let canonical = loop {
        match existing.canonicalize() {
            Ok(c) => break c,
            Err(_) => match existing.file_name() {
                Some(name) => {
                    tail.push(name.to_os_string());
                    if !existing.pop() {
                        break PathBuf::from("/");
                    }
                }
                None => break existing.clone(),
            },
        }
    };
    let mut full = canonical;
    for t in tail.into_iter().rev() {
        full.push(t);
    }
    if full == root || full.starts_with(root) {
        Ok(full)
    } else {
        Err(format!(
            "Access denied: '{p}' is outside the data directory ({})",
            root.display()
        ))
    }
}

/// Argument keys (camelCase, compared case-insensitively) that carry
/// filesystem paths in the upstream command set.
fn is_path_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.ends_with("path")
        || k.ends_with("paths")
        || k.ends_with("dir")
        || k.ends_with("directory")
        || k.ends_with("root")
        || k.ends_with("roots")
        || matches!(k.as_str(), "source" | "destination" | "dest" | "target" | "relto")
}

fn looks_like_fs_path(s: &str) -> bool {
    !s.is_empty() && !s.contains("://")
}

/// Walk a JSON argument object and check every path-like string value.
pub fn check_args(root: &Path, value: &Value) -> Result<(), String> {
    fn walk(root: &Path, v: &Value, key_is_path: bool) -> Result<(), String> {
        match v {
            Value::String(s) if key_is_path && looks_like_fs_path(s) => {
                resolve_inside(root, root, s).map(|_| ())
            }
            Value::Array(items) => items.iter().try_for_each(|i| walk(root, i, key_is_path)),
            Value::Object(map) => map
                .iter()
                .try_for_each(|(k, v)| walk(root, v, is_path_key(k))),
            _ => Ok(()),
        }
    }
    walk(root, value, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_is_rejected() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        assert!(resolve_inside(&root, &root, "a/b/c.md").is_ok());
        assert!(resolve_inside(&root, &root, "../etc/passwd").is_err());
        assert!(resolve_inside(&root, &root, "/etc/passwd").is_err());
        let inside = root.join("x/../y").to_string_lossy().to_string();
        assert!(resolve_inside(&root, &root, &inside).is_ok());
    }
}
