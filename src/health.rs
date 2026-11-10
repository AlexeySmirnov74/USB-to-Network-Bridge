//! Control-plane liveness only; USB transfer activity is never a heartbeat.
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};
pub const TIMEOUT: Duration = Duration::from_secs(120);
pub fn path(config: &Path, supervisor: bool) -> PathBuf {
    config.with_extension(if supervisor {
        "supervisor.local.json"
    } else {
        "health.local.json"
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Heartbeat {
    pub process_id: u32,
    pub role: String,
    pub sequence: u64,
    pub updated_unix_ms: u64,
    pub running: bool,
}
pub struct Lease {
    path: PathBuf,
    value: Heartbeat,
}
impl Lease {
    pub fn new(config: &Path, role: &str, supervisor: bool) -> io::Result<Self> {
        let mut lease = Self {
            path: path(config, supervisor),
            value: Heartbeat {
                process_id: std::process::id(),
                role: role.into(),
                sequence: 0,
                updated_unix_ms: 0,
                running: true,
            },
        };
        lease.beat()?;
        Ok(lease)
    }
    pub fn beat(&mut self) -> io::Result<()> {
        self.value.sequence = self.value.sequence.wrapping_add(1);
        self.value.updated_unix_ms = crate::recovery::timestamp();
        crate::settings::save(&self.path, &self.value)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.value.running = false;
        self.value.updated_unix_ms = crate::recovery::timestamp();
        let _ = crate::settings::save(&self.path, &self.value);
    }
}
pub fn fresh(updated: u64, now: u64) -> bool {
    updated <= now.saturating_add(5000) && now.saturating_sub(updated) <= TIMEOUT.as_millis() as u64
}
/// Compare sequence progress using monotonic time. Old/PID-reused files cannot keep a child alive.
pub struct Progress {
    last_poll: Duration,
    last_progress: Duration,
    sequence: Option<u64>,
}
impl Progress {
    pub fn new(now: Duration) -> Self {
        Self {
            last_poll: now,
            last_progress: now,
            sequence: None,
        }
    }
    pub fn stalled(
        &mut self,
        now: Duration,
        pid: u32,
        role: &str,
        heartbeat: Option<&Heartbeat>,
    ) -> bool {
        // Resume after sleep or a long scheduler pause gets a complete new grace period.
        if now.saturating_sub(self.last_poll) > Duration::from_secs(20) {
            self.last_progress = now;
        }
        self.last_poll = now;
        if let Some(h) = heartbeat.filter(|h| h.running && h.process_id == pid && h.role == role)
            && self.sequence != Some(h.sequence)
        {
            self.sequence = Some(h.sequence);
            self.last_progress = now;
        }
        now.saturating_sub(self.last_progress) >= TIMEOUT
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn beat(pid: u32, seq: u64) -> Heartbeat {
        Heartbeat {
            process_id: pid,
            role: "client".into(),
            sequence: seq,
            updated_unix_ms: 1,
            running: true,
        }
    }
    #[test]
    fn frozen_or_wrong_process_heartbeats_cannot_hide_a_hang() {
        let mut p = Progress::new(Duration::ZERO);
        for second in 0..120 {
            let h = beat(7, 1);
            assert!(!p.stalled(Duration::from_secs(second), 7, "client", Some(&h)));
        }
        assert!(p.stalled(Duration::from_secs(120), 7, "client", Some(&beat(8, 200))));
    }
    #[test]
    fn continuing_progress_and_resume_do_not_restart_a_healthy_agent() {
        let mut p = Progress::new(Duration::ZERO);
        for second in 0..360 {
            assert!(!p.stalled(
                Duration::from_secs(second),
                7,
                "client",
                Some(&beat(7, second))
            ));
        }
        assert!(!p.stalled(Duration::from_secs(3600), 7, "client", None));
        assert!(!p.stalled(Duration::from_secs(3601), 7, "client", Some(&beat(7, 360))));
    }
    #[test]
    fn stopped_and_future_statuses_are_not_current_connections() {
        assert!(fresh(1000, 2000));
        assert!(!fresh(1, 180000));
        assert!(!fresh(180000, 1000));
    }
}

pub fn live(heartbeat: &Heartbeat, role: &str, now: u64) -> bool {
    heartbeat.running && heartbeat.role == role && fresh(heartbeat.updated_unix_ms, now)
}
#[cfg(test)]
mod live_tests {
    use super::*;
    #[test]
    fn role_and_supervisor_freshness_are_required_in_addition_to_a_live_process() {
        let mut h = Heartbeat {
            process_id: 7,
            role: "server".into(),
            sequence: 1,
            updated_unix_ms: 1,
            running: true,
        };
        assert!(!live(&h, "server", 180000));
        h.updated_unix_ms = 180000;
        assert!(live(&h, "server", 180000));
        assert!(!live(&h, "client", 180000));
        h.running = false;
        assert!(!live(&h, "server", 180000));
    }
}
