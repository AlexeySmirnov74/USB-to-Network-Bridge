//! Structured development events. USB payloads and authentication material are excluded.
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Mutex, OnceLock},
};
struct Logger {
    path: PathBuf,
    component: String,
    lock: Mutex<()>,
    sequence: AtomicU64,
}
static LOGGER: OnceLock<Logger> = OnceLock::new();
const LIMIT: u64 = 32 * 1024 * 1024;
pub fn init(config: &Path, component: &str) {
    let primary = config
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("logs");
    let fallback = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("USB to Network Bridge")
        .join("logs");
    let path = writable_log_path(&primary, &fallback, component);
    let _ = LOGGER.set(Logger {
        path,
        component: component.into(),
        lock: Mutex::new(()),
        sequence: AtomicU64::new(0),
    });
    event(
        "process.start",
        json!({"version":crate::APP_VERSION,"config":config,"pid":std::process::id()}),
    );
}
fn writable_log_path(primary: &Path, fallback: &Path, component: &str) -> PathBuf {
    let name = format!("{component}.debug.jsonl");
    for directory in [primary, fallback] {
        if fs::create_dir_all(directory).is_ok() {
            let path = directory.join(&name);
            if OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .is_ok()
            {
                return path;
            }
        }
    }
    fallback.join(name)
}
fn redact(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                let key = key.to_ascii_lowercase();
                if ["password", "secret", "authorization", "credential", "token"]
                    .iter()
                    .any(|s| key.contains(s))
                {
                    *value = Value::String("[REDACTED]".into())
                } else {
                    redact(value)
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact(value)
            }
        }
        _ => {}
    }
}
pub fn event(name: &str, mut fields: Value) {
    let Some(logger) = LOGGER.get() else { return };
    redact(&mut fields);
    let Ok(_lock) = logger.lock.lock() else {
        return;
    };
    if fs::metadata(&logger.path).is_ok_and(|m| m.len() >= LIMIT) {
        for index in (1..=4).rev() {
            let old = if index == 1 {
                logger.path.clone()
            } else {
                logger.path.with_extension(format!("jsonl.{}", index - 1))
            };
            let next = logger.path.with_extension(format!("jsonl.{index}"));
            let _ = fs::remove_file(&next);
            let _ = fs::rename(old, next);
        }
    }
    let row = json!({"time_unix_ms":crate::recovery::timestamp(),"sequence":logger.sequence.fetch_add(1,Ordering::Relaxed),"pid":std::process::id(),"thread":std::thread::current().name().unwrap_or("worker"),"component":logger.component,"event":name,"fields":fields});
    if let Ok(mut data) = serde_json::to_vec(&row) {
        data.push(b'\n');
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&logger.path)
        {
            let _ = file.write_all(&data);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protected_or_unavailable_log_directory_uses_user_location() {
        let root = std::env::temp_dir().join(format!("usbbridge-logs-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let blocked = root.join("not-a-directory");
        fs::write(&blocked, b"unchanged").unwrap();
        let local = root.join("user-logs");
        let path = writable_log_path(&blocked, &local, "server-gui");
        assert_eq!(path, local.join("server-gui.debug.jsonl"));
        assert!(path.is_file());
        assert_eq!(fs::read(&blocked).unwrap(), b"unchanged");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn nested_authentication_is_never_serialized() {
        let mut v = json!({"proxy":{"password":"example-secret","authorization":"Basic example","credential_file":"private"},"busid":"1-2"});
        redact(&mut v);
        let text = v.to_string();
        assert!(!text.contains("example-secret"));
        assert!(!text.contains("Basic example"));
        assert_eq!(v["busid"], "1-2");
    }
}
