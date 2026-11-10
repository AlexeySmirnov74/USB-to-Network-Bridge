//! Persistent role supervision. Only the controller child started here may be terminated.
use crate::{
    desktop::Role,
    health::{self, Heartbeat, Lease, Progress},
    instance, settings,
};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io,
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    System::Threading::{OpenProcess, WaitForSingleObject},
};
fn role_name(role: Role) -> &'static str {
    if role == Role::Server {
        "Server"
    } else {
        "Client"
    }
}
fn mode(role: Role) -> &'static str {
    if role == Role::Server {
        "--host"
    } else {
        "--client"
    }
}
fn pause(stop: &AtomicBool, duration: Duration) {
    let end = Instant::now() + duration;
    while !stop.load(Ordering::Acquire) && Instant::now() < end {
        thread::sleep(
            end.saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100)),
        );
    }
}
fn spawn(config: &Path, role: Role) -> io::Result<Child> {
    let stderr = config.with_extension("agent-errors.log");
    if fs::metadata(&stderr).is_ok_and(|m| m.len() > 1024 * 1024) {
        let previous = config.with_extension("agent-errors.previous.log");
        let _ = fs::remove_file(&previous);
        fs::rename(&stderr, previous)?;
    }
    Command::new(std::env::current_exe()?.with_file_name("usbbridge-agent.exe"))
        .arg(mode(role))
        .arg("--config")
        .arg(config)
        .arg("--supervised-by")
        .arg(std::process::id().to_string())
        .env("RUST_BACKTRACE", "1")
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            OpenOptions::new().create(true).append(true).open(stderr)?,
        ))
        .spawn()
}
pub fn run(config: &Path, role: Role, stop: Arc<AtomicBool>) -> io::Result<()> {
    let name = role_name(role);
    let wire_role = name.to_ascii_lowercase();
    crate::diagnostics::init(config, &format!("{wire_role}-supervisor"));
    let Some(_instance) = instance::acquire(name, "Supervisor")? else {
        return Ok(());
    };
    let _single = settings::acquire_file_lock(&config.with_extension("supervisor.lock"))?;
    let mut lease = Lease::new(config, &wire_role, true)?;
    let mut child: Option<Child> = None;
    let started = Instant::now();
    let mut progress = Progress::new(started.elapsed());
    let mut next_start = Instant::now();
    let mut attempt = 0u64;
    crate::diagnostics::event(
        "supervisor.start",
        json!({"role":wire_role,"heartbeat_timeout_seconds":health::TIMEOUT.as_secs()}),
    );
    while !stop.load(Ordering::Acquire) {
        lease.beat()?;
        if let Some(process) = child.as_mut() {
            let pid = process.id();
            if let Some(exit) = process.try_wait()? {
                crate::diagnostics::event(
                    "supervisor.child.exit",
                    json!({"child_pid":pid,"exit_code":exit.code(),"role":wire_role,"attempt":attempt}),
                );
                child = None;
                next_start = Instant::now() + Duration::from_secs(3);
            } else {
                let heartbeat = settings::read::<Heartbeat>(&health::path(config, false)).ok();
                if progress.stalled(started.elapsed(), pid, &wire_role, heartbeat.as_ref()) {
                    crate::diagnostics::event(
                        "supervisor.child.unresponsive",
                        json!({"child_pid":pid,"role":wire_role,"heartbeat":heartbeat,"reason":"control heartbeat stopped advancing"}),
                    );
                    // The retained Child handle identifies our process, independent of PID reuse.
                    process.kill()?;
                    let exit = process.wait()?;
                    crate::diagnostics::event(
                        "supervisor.child.restarted",
                        json!({"child_pid":pid,"exit_code":exit.code(),"reason":"heartbeat_timeout"}),
                    );
                    child = None;
                    next_start = Instant::now() + Duration::from_secs(3);
                }
            }
        }
        if child.is_none() && Instant::now() >= next_start {
            // Respect a controller started manually; never terminate a process we do not own.
            if !instance::running(name, "Control")? {
                attempt = attempt.saturating_add(1);
                match spawn(config, role) {
                    Ok(process) => {
                        crate::diagnostics::event(
                            "supervisor.child.start",
                            json!({"child_pid":process.id(),"role":wire_role,"attempt":attempt}),
                        );
                        child = Some(process);
                        progress = Progress::new(started.elapsed());
                    }
                    Err(error) => crate::diagnostics::event(
                        "supervisor.spawn.failed",
                        json!({"role":wire_role,"error":error.to_string(),"attempt":attempt}),
                    ),
                }
            }
            next_start = Instant::now() + Duration::from_secs(5);
        }
        pause(&stop, Duration::from_secs(2));
    }
    if let Some(mut process) = child {
        let _ = process.kill();
        let _ = process.wait();
    }
    crate::diagnostics::event("supervisor.stop", json!({"role":wire_role}));
    Ok(())
}
/// A controller exits if its supervisor disappears, without detaching kernel USB sessions.
pub struct ParentWatch {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl ParentWatch {
    pub fn start(args: &[String], stop: Arc<AtomicBool>) -> io::Result<Option<Self>> {
        let Some(pair) = args.windows(2).find(|p| p[0] == "--supervised-by") else {
            return Ok(None);
        };
        let pid = pair[1]
            .parse::<u32>()
            .map_err(|_| io::Error::other("invalid supervisor PID"))?;
        let process = unsafe { OpenProcess(0x00100000, 0, pid) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = process as usize;
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                let result = unsafe { WaitForSingleObject(handle as _, 200) };
                if result != 258 {
                    crate::diagnostics::event(
                        "controller.supervisor_lost",
                        json!({"supervisor_pid":pid,"wait_result":result}),
                    );
                    worker_stop.store(true, Ordering::Release);
                    break;
                }
            }
            unsafe { CloseHandle(handle as _) };
        });
        Ok(Some(Self {
            stop,
            worker: Some(worker),
        }))
    }
}
impl Drop for ParentWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
pub fn status(config: &Path, role: Role) -> io::Result<Value> {
    let name = role_name(role);
    let heartbeat = settings::read::<Heartbeat>(&health::path(config, false)).ok();
    let supervisor = settings::read::<Heartbeat>(&health::path(config, true)).ok();
    let now = crate::recovery::timestamp();
    let controller_running = instance::running(name, "Control")?;
    let supervisor_running = instance::running(name, "Supervisor")?;
    Ok(json!({
        "controller_running":controller_running,"supervisor_running":supervisor_running,
        "healthy":controller_running && supervisor_running && heartbeat.as_ref().is_some_and(|h|health::live(h,&name.to_ascii_lowercase(),now)) && supervisor.as_ref().is_some_and(|h|health::live(h,&name.to_ascii_lowercase(),now)),
        "controller":heartbeat,"supervisor":supervisor,"heartbeat_timeout_seconds":health::TIMEOUT.as_secs()
    }))
}

pub fn check(config: &Path, role: Role) -> io::Result<()> {
    let name = role_name(role);
    crate::diagnostics::init(config, &format!("{}-health", name.to_ascii_lowercase()));
    let Some(_instance) = instance::acquire(name, "HealthCheck")? else {
        return Ok(());
    };
    let snapshot = status(config, role)?;
    let now = crate::recovery::timestamp();
    let supervisor_fresh = snapshot["supervisor"]["updated_unix_ms"]
        .as_u64()
        .is_some_and(|updated| health::fresh(updated, now))
        && snapshot["supervisor"]["running"].as_bool() == Some(true);
    if snapshot["healthy"].as_bool() == Some(true) && supervisor_fresh {
        crate::diagnostics::event("health.check", json!({"role":name,"healthy":true}));
        return Ok(());
    }
    crate::diagnostics::event(
        "health.recovery.begin",
        json!({"role":name,"health":snapshot}),
    );
    let root = std::env::current_exe()?
        .parent()
        .ok_or_else(|| io::Error::other("Runtime directory missing"))?
        .to_path_buf();
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(root.join("Restart-AgentSupervisor.ps1"))
        .args(["-Role", name, "-Config"])
        .arg(config)
        .creation_flags(0x08000000)
        .output()?;
    let result = String::from_utf8_lossy(&output.stdout);
    crate::diagnostics::event(
        "health.recovery.result",
        json!({"role":name,"success":output.status.success(),"result":result,"error":String::from_utf8_lossy(&output.stderr)}),
    );
    if !output.status.success() {
        return Err(io::Error::other(
            "Supervisor task recovery failed; see health log",
        ));
    }
    Ok(())
}
