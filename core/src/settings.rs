//! Small persistent key/value store for UI preferences (sort orders, open panels, last page…),
//! kept in ~/.config/crate/desktop-settings.json so it survives restarts and UI rewrites.
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Mutex;

pub struct Settings {
    path: Option<PathBuf>,
    data: Mutex<Map<String, Value>>,
}

impl Default for Settings {
    fn default() -> Self {
        Self::load()
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = dirs::config_dir().map(|d| d.join("crate/desktop-settings.json"));
        let data = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        Settings { path, data: Mutex::new(data) }
    }

    pub fn all(&self) -> Value {
        Value::Object(self.data.lock().unwrap().clone())
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.data.lock().unwrap().get(key).cloned()
    }

    /// Set (or, with `null`, remove) a key and write the file atomically.
    pub fn set(&self, key: &str, value: Value) {
        let snapshot = {
            let mut d = self.data.lock().unwrap();
            if value.is_null() {
                d.remove(key);
            } else {
                d.insert(key.to_string(), value);
            }
            d.clone()
        };
        if let Some(p) = &self.path {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = p.with_extension("json.tmp");
            if let Ok(b) = serde_json::to_vec_pretty(&Value::Object(snapshot)) {
                if std::fs::write(&tmp, b).is_ok() {
                    let _ = std::fs::rename(&tmp, p);
                }
            }
        }
    }
}
