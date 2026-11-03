//! Slow, identity-scoped driver transitions; the transfer path stays in Rust.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
#[derive(Clone, Serialize)]
struct Request {
    operation: &'static str,
    identity: String,
    token: Option<String>,
    process_id: u32,
}
#[derive(Deserialize)]
struct Reply {
    #[serde(rename = "Success")]
    success: bool,
    #[serde(rename = "Busy")]
    busy: bool,
    #[serde(default, rename = "Error")]
    error: Option<String>,
}
pub(crate) struct Broker {
    runtime: PathBuf,
    helper: PathBuf,
    cleanup: Mutex<Cleanup>,
}
#[derive(Default)]
struct Cleanup {
    // Remain reserved during capture AND while a release is queued/in flight.
    identities: HashSet<String>,
    pending: Vec<Request>,
}
pub(crate) struct Lease {
    broker: Arc<Broker>,
    request: Request,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.broker
            .cleanup
            .lock()
            .unwrap()
            .pending
            .push(self.request.clone());
    }
}
impl Broker {
    pub(crate) fn new(runtime: &Path, helper: &Path) -> Arc<Self> {
        Arc::new(Self {
            runtime: runtime.into(),
            helper: helper.into(),
            cleanup: Mutex::new(Cleanup::default()),
        })
    }
    fn invoke(&self, request: &Request, try_lock: bool) -> io::Result<bool> {
        let file = self.runtime.join(format!(
            "native-capture.{}.request.tmp",
            uuid::Uuid::new_v4()
        ));
        let result = file.with_file_name(format!(
            "{}.result.json",
            file.file_name().unwrap().to_string_lossy()
        ));
        struct Files(PathBuf, PathBuf);
        impl Drop for Files {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
                let _ = std::fs::remove_file(&self.1);
            }
        }
        let _files = Files(file.clone(), result.clone());
        crate::settings::save(&file, request)?;
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&self.helper)
            .arg("-RuntimeDirectory")
            .arg(&self.runtime)
            .arg("-RequestFile")
            .arg(&file)
            .creation_flags(0x08000000)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if try_lock {
            command.arg("-TryLock");
        }
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(35);
        loop {
            if let Some(status) = child.try_wait()? {
                let reply: Reply = crate::settings::read(&result)?;
                if !status.success() || !reply.success {
                    return Err(io::Error::other(
                        reply
                            .error
                            .unwrap_or_else(|| "Native capture helper failed".into()),
                    ));
                }
                return Ok(!reply.busy);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Native capture helper timeout",
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    pub(crate) fn acquire(self: &Arc<Self>, identity: &str) -> io::Result<Lease> {
        if !self
            .cleanup
            .lock()
            .unwrap()
            .identities
            .insert(identity.to_ascii_lowercase())
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Native capture is active or awaiting cleanup for this device",
            ));
        }
        let mut request = Request {
            operation: "release",
            identity: identity.into(),
            token: Some(uuid::Uuid::new_v4().to_string()),
            process_id: std::process::id(),
        };
        let lease = Lease {
            broker: self.clone(),
            request: request.clone(),
        };
        request.operation = "acquire";
        if !self.invoke(&request, false)? {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        Ok(lease)
    }
    pub(crate) fn drain(&self) {
        let pending = std::mem::take(&mut self.cleanup.lock().unwrap().pending);
        for request in pending {
            match self.invoke(&request, true) {
                Ok(true) => {
                    self.cleanup
                        .lock()
                        .unwrap()
                        .identities
                        .remove(&request.identity.to_ascii_lowercase());
                }
                Ok(false) => self.cleanup.lock().unwrap().pending.push(request),
                Err(error) => {
                    crate::diagnostics::event(
                        "native.capture.restore.failed",
                        serde_json::json!({"identity":request.identity,"error":error.to_string()}),
                    );
                    self.cleanup.lock().unwrap().pending.push(request);
                }
            }
        }
    }
    pub(crate) fn restore_idle(&self, identity: &str) {
        let request = Request {
            operation: "release",
            identity: identity.into(),
            token: None,
            process_id: std::process::id(),
        };
        if let Err(error) = self.invoke(&request, true) {
            crate::diagnostics::event(
                "native.capture.reconcile.failed",
                serde_json::json!({"identity":identity,"error":error.to_string()}),
            );
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_acquisition_queues_cleanup_and_busy_release_is_retried_after_drop() {
        let runtime =
            std::env::temp_dir().join(format!("usbbridge-capture-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&runtime).unwrap();
        let helper = runtime.join("fake.ps1");
        std::fs::write(&helper,r#"param([string]$RuntimeDirectory,[string]$RequestFile,[switch]$TryLock)
$request=Get-Content $RequestFile -Raw|ConvertFrom-Json
$request|ConvertTo-Json -Compress|Add-Content (Join-Path $RuntimeDirectory 'events.tmp')
$busy=$false;$success=$true
if($request.operation-eq 'acquire'){$success=$false}
if($request.operation-eq 'release'-and -not(Test-Path (Join-Path $RuntimeDirectory 'busy.tmp'))){$busy=$true;'seen'|Set-Content (Join-Path $RuntimeDirectory 'busy.tmp')}
@{Success=$success;Busy=$busy;Error='deliberate test failure'}|ConvertTo-Json|Set-Content ($RequestFile+'.result.json')
if(-not $success){exit 1}
"#).unwrap();
        let broker = Broker::new(&runtime, &helper);
        assert!(broker.acquire("USB\\test").is_err());
        assert_eq!(broker.cleanup.lock().unwrap().pending.len(), 1);
        for _ in 0..1000 {
            assert_eq!(
                broker.acquire("usb\\TEST").err().unwrap().kind(),
                io::ErrorKind::WouldBlock
            );
        }
        assert_eq!(broker.cleanup.lock().unwrap().pending.len(), 1);
        let events = || {
            std::fs::read_to_string(runtime.join("events.tmp"))
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(events().len(), 1); // Cleanup did not run from the failed-open stack.
        broker.drain();
        assert_eq!(broker.cleanup.lock().unwrap().pending.len(), 1);
        assert!(broker.acquire("USB\\test").is_err());
        broker.drain();
        assert!(broker.cleanup.lock().unwrap().pending.is_empty());
        assert!(broker.cleanup.lock().unwrap().identities.is_empty());
        let events = events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["operation"], "acquire");
        assert_eq!(events[1]["operation"], "release");
        assert_eq!(events[0]["token"], events[2]["token"]);
        for file in std::fs::read_dir(&runtime).unwrap() {
            std::fs::remove_file(file.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(runtime).unwrap();
    }
}
