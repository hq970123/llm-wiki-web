//! Server-side replacement for tauri-plugin-store.
//!
//! Stores are flat JSON objects persisted at `<app_data_dir>/<name>` —
//! exactly where the desktop plugin keeps them, so Rust code that reads
//! `app-state.json` directly (agent config, proxy config, api_server)
//! sees the same data.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::AppState;

pub struct Stores {
    dir: PathBuf,
    cache: Mutex<HashMap<String, Map<String, Value>>>,
}

#[derive(Deserialize)]
pub struct StoreReq {
    name: String,
    op: String,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    defaults: Option<Map<String, Value>>,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.')
}

impl Stores {
    pub fn new(dir: PathBuf) -> Self {
        Stores {
            dir,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn load_locked<'a>(
        &self,
        cache: &'a mut HashMap<String, Map<String, Value>>,
        name: &str,
    ) -> &'a mut Map<String, Value> {
        cache.entry(name.to_string()).or_insert_with(|| {
            std::fs::read_to_string(self.dir.join(name))
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default()
        })
    }

    fn persist(&self, name: &str, map: &Map<String, Value>) -> Result<(), String> {
        let path = self.dir.join(name);
        let tmp = self.dir.join(format!(".{name}.tmp"));
        let data = serde_json::to_vec_pretty(map).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    fn op(&self, req: StoreReq) -> Result<Value, String> {
        if !valid_name(&req.name) {
            return Err("invalid store name".into());
        }
        let mut cache = self.cache.lock().unwrap();
        // Reload from disk if requested (another writer, e.g. a backend command).
        if req.op == "reload" {
            cache.remove(&req.name);
        }
        let map = self.load_locked(&mut cache, &req.name);
        if let Some(defaults) = req.defaults {
            for (k, v) in defaults {
                map.entry(k).or_insert(v);
            }
        }
        let key = req.key.unwrap_or_default();
        let out = match req.op.as_str() {
            "load" | "reload" => json!(true),
            "get" => map.get(&key).cloned().unwrap_or(Value::Null),
            "has" => json!(map.contains_key(&key)),
            "keys" => json!(map.keys().cloned().collect::<Vec<_>>()),
            "values" => json!(map.values().cloned().collect::<Vec<_>>()),
            "entries" => json!(map
                .iter()
                .map(|(k, v)| json!([k, v]))
                .collect::<Vec<_>>()),
            "length" => json!(map.len()),
            "set" => {
                map.insert(key, req.value.unwrap_or(Value::Null));
                let snapshot = map.clone();
                self.persist(&req.name, &snapshot)?;
                json!(true)
            }
            "delete" => {
                let existed = map.remove(&key).is_some();
                let snapshot = map.clone();
                self.persist(&req.name, &snapshot)?;
                json!(existed)
            }
            "clear" | "reset" => {
                map.clear();
                self.persist(&req.name, &Map::new())?;
                json!(true)
            }
            "save" => {
                let snapshot = map.clone();
                self.persist(&req.name, &snapshot)?;
                json!(true)
            }
            other => return Err(format!("unknown store op {other}")),
        };
        Ok(out)
    }
}

pub async fn handle_store(State(st): State<AppState>, Json(req): Json<StoreReq>) -> Response {
    let stores = st.stores.clone();
    let is_app_state = req.name == "app-state.json" && matches!(req.op.as_str(), "set" | "delete" | "clear");
    match tokio::task::spawn_blocking(move || stores.op(req)).await {
        Ok(Ok(v)) => {
            if is_app_state {
                // api_server caches app-state.json; keep it coherent.
                crate::api_server::invalidate_config_cache();
            }
            Json(json!({ "value": v })).into_response()
        }
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}
