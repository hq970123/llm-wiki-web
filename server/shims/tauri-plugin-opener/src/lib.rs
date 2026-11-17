//! Headless stand-in for tauri-plugin-opener: there is no desktop to open
//! files or folders on, so every call reports a clear error.
use tauri::AppHandle;

pub struct Opener;

#[derive(Debug)]
pub struct Error(String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

fn unsupported() -> Error {
    Error("Opening files or folders on the server desktop is not available in the web version".into())
}

impl Opener {
    pub fn open_path<P: AsRef<str>, S: AsRef<str>>(&self, _path: P, _with: Option<S>) -> Result<(), Error> {
        Err(unsupported())
    }
    pub fn open_url<P: AsRef<str>, S: AsRef<str>>(&self, _url: P, _with: Option<S>) -> Result<(), Error> {
        Err(unsupported())
    }
    pub fn reveal_item_in_dir<P: AsRef<std::path::Path>>(&self, _path: P) -> Result<(), Error> {
        Err(unsupported())
    }
}

pub trait OpenerExt {
    fn opener(&self) -> Opener;
}

impl OpenerExt for AppHandle {
    fn opener(&self) -> Opener {
        Opener
    }
}
