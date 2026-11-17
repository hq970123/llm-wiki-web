//! Headless stand-in for the parts of the `tauri` crate that LLM Wiki's
//! backend (src-tauri/src) touches outside of `run()`.
//!
//! The web server compiles the upstream `src-tauri/src/lib.rs` unchanged
//! (apart from a few `cfg(not(feature = "web"))` gates) against this crate,
//! so `#[tauri::command]`, `AppHandle`, `State`, `Emitter`, `Manager` and
//! `async_runtime` keep working without a webview, GTK or a display.
//!
//! `#[tauri::command]` here leaves the function untouched and additionally
//! generates `__web_cmd_<name>(app, args) -> BoxFuture<Result<Value, Value>>`
//! which the HTTP layer dispatches to from `POST /api/invoke/<name>`.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub use tauri_web_macros::command;

// ---------------------------------------------------------------- errors

#[derive(Debug)]
pub enum Error {
    Msg(String),
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Msg(m) => f.write_str(m),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Marker type so `AppHandle<Wry>`-style signatures (if any) still parse.
pub struct Wry;
pub trait Runtime: Send + Sync + 'static {}
impl Runtime for Wry {}

// ---------------------------------------------------------------- events

/// One backend event, as forwarded to `GET /api/events` (SSE).
#[derive(Clone, Debug)]
pub struct EmittedEvent {
    pub event: String,
    pub payload: serde_json::Value,
}

// ---------------------------------------------------------------- app handle

struct Inner {
    states: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    events: tokio::sync::broadcast::Sender<EmittedEvent>,
    app_data_dir: PathBuf,
    app_config_dir: PathBuf,
    resource_dir: PathBuf,
}

#[derive(Clone)]
pub struct AppHandle {
    inner: Arc<Inner>,
}

impl AppHandle {
    /// Build the headless app handle. `app_data_dir` replaces Tauri's
    /// per-user `com.llmwiki.app` directory; `resource_dir` is where
    /// bundled resources (pdfium, mcp-server) are looked up.
    pub fn new_headless(app_data_dir: PathBuf, resource_dir: PathBuf) -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(4096);
        AppHandle {
            inner: Arc::new(Inner {
                states: RwLock::new(HashMap::new()),
                events: tx,
                app_config_dir: app_data_dir.clone(),
                app_data_dir,
                resource_dir,
            }),
        }
    }

    /// Subscribe to every event emitted through [`Emitter::emit`].
    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<EmittedEvent> {
        self.inner.events.subscribe()
    }

    /// Emit an already-serialized payload (used by the HTTP `/api/emit`).
    pub fn emit_value(&self, event: &str, payload: serde_json::Value) {
        let _ = self.inner.events.send(EmittedEvent {
            event: event.to_string(),
            payload,
        });
    }

    pub fn app_handle(&self) -> &AppHandle {
        self
    }
}

pub struct PathResolver {
    app: AppHandle,
}

impl PathResolver {
    pub fn app_data_dir(&self) -> Result<PathBuf> {
        Ok(self.app.inner.app_data_dir.clone())
    }
    pub fn app_config_dir(&self) -> Result<PathBuf> {
        Ok(self.app.inner.app_config_dir.clone())
    }
    pub fn app_local_data_dir(&self) -> Result<PathBuf> {
        Ok(self.app.inner.app_data_dir.clone())
    }
    pub fn app_cache_dir(&self) -> Result<PathBuf> {
        Ok(self.app.inner.app_data_dir.join("cache"))
    }
    pub fn app_log_dir(&self) -> Result<PathBuf> {
        Ok(self.app.inner.app_data_dir.join("logs"))
    }
    pub fn resource_dir(&self) -> Result<PathBuf> {
        Ok(self.app.inner.resource_dir.clone())
    }
    pub fn home_dir(&self) -> Result<PathBuf> {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| Error::Msg("HOME is not set".into()))
    }
    pub fn temp_dir(&self) -> Result<PathBuf> {
        Ok(std::env::temp_dir())
    }
}

/// Managed state handle. Owns an `Arc<T>`; the lifetime parameter only
/// exists so upstream signatures like `State<'_, T>` type-check.
pub struct State<'r, T: Send + Sync + 'static> {
    value: Arc<T>,
    _marker: PhantomData<&'r ()>,
}

impl<'r, T: Send + Sync + 'static> State<'r, T> {
    pub fn inner(&self) -> &T {
        &self.value
    }
}

impl<'r, T: Send + Sync + 'static> std::ops::Deref for State<'r, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<'r, T: Send + Sync + 'static> Clone for State<'r, T> {
    fn clone(&self) -> Self {
        State {
            value: self.value.clone(),
            _marker: PhantomData,
        }
    }
}

pub trait Manager {
    fn app_handle_ref(&self) -> &AppHandle;

    fn manage<T: Send + Sync + 'static>(&self, value: T) -> bool {
        let app = self.app_handle_ref();
        let mut map = app.inner.states.write().unwrap();
        let id = TypeId::of::<T>();
        if map.contains_key(&id) {
            return false;
        }
        map.insert(id, Arc::new(value));
        true
    }

    fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>> {
        let app = self.app_handle_ref();
        let map = app.inner.states.read().unwrap();
        let any = map.get(&TypeId::of::<T>())?.clone();
        let value = any.downcast::<T>().ok()?;
        Some(State {
            value,
            _marker: PhantomData,
        })
    }

    fn state<T: Send + Sync + 'static>(&self) -> State<'_, T> {
        self.try_state::<T>().unwrap_or_else(|| {
            panic!(
                "state() called before manage() for type {}",
                std::any::type_name::<T>()
            )
        })
    }

    fn path(&self) -> PathResolver {
        PathResolver {
            app: self.app_handle_ref().clone(),
        }
    }
}

impl Manager for AppHandle {
    fn app_handle_ref(&self) -> &AppHandle {
        self
    }
}

pub trait Emitter {
    fn emit<S: serde::Serialize + Clone>(&self, event: &str, payload: S) -> Result<()>;
}

impl Emitter for AppHandle {
    fn emit<S: serde::Serialize + Clone>(&self, event: &str, payload: S) -> Result<()> {
        let payload = serde_json::to_value(payload).map_err(|e| Error::Msg(e.to_string()))?;
        self.emit_value(event, payload);
        Ok(())
    }
}

// ---------------------------------------------------------------- runtime

pub mod async_runtime {
    use std::future::Future;
    use std::sync::OnceLock;

    pub use tokio::task::JoinHandle;

    static HANDLE: OnceLock<tokio::runtime::Handle> = OnceLock::new();

    /// Must be called once from inside the server's tokio runtime.
    pub fn set_handle(handle: tokio::runtime::Handle) {
        let _ = HANDLE.set(handle);
    }

    pub fn handle() -> tokio::runtime::Handle {
        tokio::runtime::Handle::try_current()
            .ok()
            .or_else(|| HANDLE.get().cloned())
            .expect("tokio runtime handle not initialised")
    }

    pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        handle().spawn(future)
    }

    pub fn spawn_blocking<F, R>(func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        handle().spawn_blocking(func)
    }

    /// Blocks the current (non-runtime) thread on a future. Upstream only
    /// calls this from the tiny_http worker threads of api_server.rs.
    pub fn block_on<F: Future>(future: F) -> F::Output {
        let h = HANDLE.get().cloned().expect("tokio runtime handle not initialised");
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| h.block_on(future))
        } else {
            h.block_on(future)
        }
    }
}

// ---------------------------------------------------------------- command glue

#[doc(hidden)]
pub mod __private {
    use super::*;
    pub use futures::future::BoxFuture;
    pub use serde_json::Value;

    pub type InvokeResult = std::result::Result<Value, Value>;

    /// Deserialize a named command argument from the JSON args object.
    /// Missing keys are treated as `null` so `Option<T>` args work.
    pub fn arg<T: serde::de::DeserializeOwned>(
        args: &Value,
        key: &str,
        command: &str,
    ) -> std::result::Result<T, Value> {
        let raw = args.get(key).cloned().unwrap_or(Value::Null);
        let was_missing = raw.is_null();
        serde_json::from_value(raw).map_err(|e| {
            if was_missing {
                Value::String(format!(
                    "invalid args `{key}` for command `{command}`: command {command} missing required key {key}"
                ))
            } else {
                Value::String(format!("invalid args `{key}` for command `{command}`: {e}"))
            }
        })
    }

    pub fn state_owned<T: Send + Sync + 'static>(
        app: &AppHandle,
    ) -> std::result::Result<State<'static, T>, Value> {
        let map = app.inner.states.read().unwrap();
        let any = map.get(&TypeId::of::<T>()).cloned().ok_or_else(|| {
            Value::String(format!(
                "state not managed for type {}",
                std::any::type_name::<T>()
            ))
        })?;
        let value = any
            .downcast::<T>()
            .map_err(|_| Value::String("state type mismatch".into()))?;
        Ok(State {
            value,
            _marker: PhantomData,
        })
    }

    pub fn ok_value<T: serde::Serialize>(v: T) -> InvokeResult {
        serde_json::to_value(v).map_err(|e| Value::String(e.to_string()))
    }

    pub fn err_value<E: serde::Serialize>(e: E) -> InvokeResult {
        Err(serde_json::to_value(e).unwrap_or_else(|e| Value::String(e.to_string())))
    }

    pub fn join_error(e: tokio::task::JoinError) -> Value {
        if e.is_panic() {
            let p = e.into_panic();
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".into());
            Value::String(format!("command panicked: {msg}"))
        } else {
            Value::String(format!("command task failed: {e}"))
        }
    }
}
