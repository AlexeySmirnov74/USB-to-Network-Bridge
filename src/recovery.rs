//! Reconciliation of remembered USB identities with live driver state.
//! Management failures never forcibly tear down an existing USB connection.
use crate::{settings::ClientSettings, wire::Device};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io, time::Duration};

#[derive(Clone, Debug)]
pub struct Port {
    pub number: i32,
    pub server: String,
    pub busid: String,
}
pub trait Driver {
    fn ports(&mut self) -> io::Result<Vec<Port>>;
    fn list(&mut self, server: &str) -> io::Result<Vec<Device>>;
    fn attach(&mut self, server: &str, busid: &str, low_latency: bool) -> io::Result<i32>;
    fn detach(&mut self, port: &Port) -> io::Result<()>;
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub identity: String,
    pub busid: String,
    pub port: Option<i32>,
    pub phase: String,
    pub error: Option<String>,
    pub retry_in_seconds: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientStatus {
    pub updated_unix_ms: u64,
    pub server: String,
    pub phase: String,
    pub error: Option<String>,
    pub devices: Vec<DeviceStatus>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionStatus {
    pub key: String,
    pub name: String,
    #[serde(flatten)]
    pub status: ClientStatus,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientReport {
    #[serde(flatten)]
    pub selected: ClientStatus,
    #[serde(default)]
    pub servers: Vec<ConnectionStatus>,
}
impl ClientStatus {
    pub fn error(server: &str, error: impl ToString) -> Self {
        Self {
            updated_unix_ms: timestamp(),
            server: server.into(),
            phase: "waiting".into(),
            error: Some(error.to_string()),
            devices: Vec::new(),
        }
    }
}
pub fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
#[derive(Default)]
struct Retry {
    last_busid: String,
    failures: u32,
    due: Duration,
    missing: u32,
}
impl Retry {
    fn fail(&mut self, now: Duration, maximum: u64) {
        self.failures = self.failures.saturating_add(1);
        let seconds = 2u64.saturating_pow(self.failures.min(8)).min(maximum);
        self.due = now + Duration::from_secs(seconds);
    }
}
#[derive(Default)]
pub struct Reconciler {
    retries: HashMap<String, Retry>,
}
impl Reconciler {
    pub fn tick(
        &mut self,
        config: &ClientSettings,
        now: Duration,
        driver: &mut impl Driver,
    ) -> ClientStatus {
        let mut status = ClientStatus {
            updated_unix_ms: timestamp(),
            server: config.server.clone(),
            phase: "connected".into(),
            error: None,
            devices: Vec::new(),
        };
        let ports = match driver.ports() {
            Ok(ports) => ports,
            Err(error) => return ClientStatus::error(&config.server, error),
        };
        if config.paused {
            self.retries.clear();
            status.phase = "paused".into();
            for port in ports
                .iter()
                .filter(|p| p.server.eq_ignore_ascii_case(&config.server))
            {
                if let Err(error) = driver.detach(port) {
                    status.error = Some(error.to_string());
                }
            }
            return status;
        }
        if !config.devices.iter().any(|d| d.enabled) {
            status.phase = "no_selection".into();
            return status;
        }
        let listed = driver.list(&config.server);
        if let Err(error) = &listed {
            status.error = Some(error.to_string());
        }
        for target in config.devices.iter().filter(|d| d.enabled) {
            let key = format!(
                "{}:{}",
                config.server.to_ascii_lowercase(),
                target.identity.to_ascii_lowercase()
            );
            let retry = self.retries.entry(key).or_default();
            let matching = listed.as_ref().ok().and_then(|devices| {
                devices.iter().find(|d| {
                    d.path.eq_ignore_ascii_case(&target.identity)
                        && d.vendor == target.vendor
                        && d.product == target.product
                })
            });
            let remembered_busid = if retry.last_busid.is_empty() {
                &target.busid
            } else {
                &retry.last_busid
            };
            let busid = matching
                .map(|d| d.busid.as_str())
                .unwrap_or(remembered_busid)
                .to_string();
            let active = ports.iter().find(|p| {
                p.server.eq_ignore_ascii_case(&config.server)
                    && (p.busid == busid || p.busid == target.busid)
                    && !listed.as_ref().is_ok_and(|devices| {
                        devices.iter().any(|d| {
                            d.busid == p.busid && !d.path.eq_ignore_ascii_case(&target.identity)
                        })
                    })
            });
            let mut item = DeviceStatus {
                identity: target.identity.clone(),
                busid: busid.clone(),
                port: active.map(|p| p.number),
                phase: "waiting".into(),
                error: None,
                retry_in_seconds: 0,
            };
            if listed.is_err() {
                // Keep an active port during transient management/network failure.
                item.phase = if active.is_some() {
                    "connected_unconfirmed"
                } else {
                    "waiting_network"
                }
                .into();
            } else if matching.is_none() {
                retry.missing = retry.missing.saturating_add(1);
                item.phase = "waiting_device".into();
                // Two positive inventories must agree before detaching our port.
                if retry.missing >= 2
                    && let Some(port) = active
                {
                    match driver.detach(port) {
                        Ok(()) => item.port = None,
                        Err(error) => item.error = Some(error.to_string()),
                    }
                }
            } else if active.is_some_and(|p| p.busid == busid) {
                *retry = Retry::default();
                item.phase = "connected".into();
            } else if now < retry.due {
                item.phase = "retrying".into();
                item.retry_in_seconds = (retry.due - now).as_secs().max(1);
            } else {
                retry.missing = 0;
                // A replug can change bus ID. Only retire the old matching port.
                let retired = active.map_or(Ok(()), |port| driver.detach(port));
                match retired
                    .and_then(|()| driver.attach(&config.server, &busid, target.low_latency))
                {
                    Ok(port) => {
                        *retry = Retry::default();
                        item.port = Some(port);
                        item.phase = "connected".into();
                    }
                    Err(error) => {
                        retry.fail(now, config.retry_max_seconds);
                        item.port = None;
                        item.phase = "retrying".into();
                        item.error = Some(error.to_string());
                        item.retry_in_seconds = (retry.due - now).as_secs();
                    }
                }
            }
            if matching.is_some() {
                retry.last_busid = busid;
            }
            if item.phase != "connected" {
                status.phase = "waiting".into();
            }
            status.devices.push(item);
        }
        self.retries.retain(|key, _| {
            config.devices.iter().any(|d| {
                d.enabled
                    && key
                        == &format!(
                            "{}:{}",
                            config.server.to_ascii_lowercase(),
                            d.identity.to_ascii_lowercase()
                        )
            })
        });
        status
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_report_roundtrip_keeps_connected_devices_for_each_server() {
        let status = ClientStatus {
            updated_unix_ms: timestamp(),
            server: "192.0.2.1:4500".into(),
            phase: "connected".into(),
            error: None,
            devices: vec![DeviceStatus {
                identity: "USB\\fixture".into(),
                busid: "2-4".into(),
                port: Some(1),
                phase: "connected".into(),
                error: None,
                retry_in_seconds: 0,
            }],
        };
        let report = ClientReport {
            selected: status.clone(),
            servers: vec![ConnectionStatus {
                key: "fixture".into(),
                name: "host".into(),
                status,
            }],
        };
        let bytes = serde_json::to_vec(&report).unwrap();
        let restored: ClientReport = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.servers[0].status.devices[0].port, Some(1));
        assert_eq!(restored.selected.phase, "connected");
    }
    struct Fake {
        online: bool,
        present: bool,
        device: Device,
        ports: Vec<Port>,
        attempts: u32,
        fail_attach: bool,
        detached: Vec<i32>,
    }
    impl Driver for Fake {
        fn ports(&mut self) -> io::Result<Vec<Port>> {
            Ok(self.ports.clone())
        }
        fn list(&mut self, _: &str) -> io::Result<Vec<Device>> {
            if !self.online {
                return Err(io::ErrorKind::TimedOut.into());
            }
            Ok(if self.present {
                vec![self.device.clone()]
            } else {
                Vec::new()
            })
        }
        fn attach(&mut self, server: &str, busid: &str, _: bool) -> io::Result<i32> {
            self.attempts += 1;
            if self.fail_attach {
                return Err(io::ErrorKind::ConnectionRefused.into());
            }
            self.ports.push(Port {
                number: 7,
                server: server.into(),
                busid: busid.into(),
            });
            Ok(7)
        }
        fn detach(&mut self, port: &Port) -> io::Result<()> {
            self.detached.push(port.number);
            self.ports.retain(|p| p.number != port.number);
            Ok(())
        }
    }
    fn fixture() -> (ClientSettings, Fake) {
        let device = crate::backend::mock_device();
        let mut config = ClientSettings {
            server: "127.0.0.1:3240".into(),
            ..Default::default()
        };
        config.remember(&device, false);
        (
            config,
            Fake {
                online: true,
                present: true,
                device,
                ports: Vec::new(),
                attempts: 0,
                fail_attach: false,
                detached: Vec::new(),
            },
        )
    }
    #[test]
    fn pausing_a_server_detaches_its_ports_without_changing_device_choices_or_other_servers() {
        let (mut config, mut driver) = fixture();
        driver.ports = vec![
            Port {
                number: 1,
                server: config.server.clone(),
                busid: "old-location".into(),
            },
            Port {
                number: 2,
                server: "192.0.2.2:3240".into(),
                busid: "2-4".into(),
            },
        ];
        config.paused = true;
        let target = serde_json::to_value(&config.devices).unwrap();
        let mut reconciler = Reconciler::default();
        let status = reconciler.tick(&config, Duration::ZERO, &mut driver);
        assert_eq!(status.phase, "paused");
        assert_eq!(driver.detached, [1]);
        assert_eq!(driver.ports[0].number, 2);
        assert_eq!(driver.attempts, 0);
        assert_eq!(serde_json::to_value(&config.devices).unwrap(), target);
        let status = reconciler.tick(&config, Duration::from_secs(40), &mut driver);
        assert_eq!(status.phase, "paused");
        assert_eq!(driver.attempts, 0);
        config.paused = false;
        assert_eq!(
            reconciler
                .tick(&config, Duration::from_secs(41), &mut driver)
                .phase,
            "connected"
        );
        assert_eq!(driver.attempts, 1);
        assert_eq!(driver.ports[0].number, 2);
    }
    #[test]
    fn short_network_outages_preserve_live_usb_port() {
        let (config, mut driver) = fixture();
        let mut recovery = Reconciler::default();
        recovery.tick(&config, Duration::ZERO, &mut driver);
        driver.online = false;
        let status = recovery.tick(&config, Duration::from_secs(600), &mut driver);
        assert_eq!(status.devices[0].phase, "connected_unconfirmed");
        assert!(driver.detached.is_empty());
        assert_eq!(driver.attempts, 1);
        driver.online = true;
        assert_eq!(
            recovery
                .tick(&config, Duration::from_secs(603), &mut driver)
                .devices[0]
                .phase,
            "connected"
        );
    }
    #[test]
    fn unplug_replug_reconnects_same_identity_and_ignores_replacement() {
        let (config, mut driver) = fixture();
        let mut recovery = Reconciler::default();
        recovery.tick(&config, Duration::ZERO, &mut driver);
        driver.ports.push(Port {
            number: 99,
            server: "other:3240".into(),
            busid: driver.device.busid.clone(),
        });
        driver.present = false;
        recovery.tick(&config, Duration::from_secs(3), &mut driver);
        assert!(driver.detached.is_empty());
        recovery.tick(&config, Duration::from_secs(6), &mut driver);
        assert_eq!(driver.detached, vec![7]);
        driver.present = true;
        driver.device.path.push_str("-replacement");
        recovery.tick(&config, Duration::from_secs(9), &mut driver);
        assert_eq!(driver.attempts, 1);
        driver.device.path = config.devices[0].identity.clone();
        driver.device.busid = "2-9".into();
        let status = recovery.tick(&config, Duration::from_secs(12), &mut driver);
        assert_eq!(status.devices[0].phase, "connected");
        assert_eq!(driver.attempts, 2);
        assert_eq!(driver.ports.len(), 2);
        assert!(driver.ports.iter().any(|p| p.number == 99));
    }
    #[test]
    fn retry_is_bounded_in_rate_not_in_number_of_attempts_and_survives_restart() {
        let (config, mut driver) = fixture();
        let mut recovery = Reconciler::default();
        driver.fail_attach = true;
        for seconds in 0..1000 {
            recovery.tick(&config, Duration::from_secs(seconds), &mut driver);
        }
        assert!(driver.attempts > 25 && driver.attempts < 40);
        driver.fail_attach = false;
        let mut restarted = Reconciler::default();
        assert_eq!(
            restarted.tick(&config, Duration::ZERO, &mut driver).devices[0].phase,
            "connected"
        );
    }
    #[test]
    fn replugged_location_is_remembered_and_replacement_session_is_untouched() {
        let (config, mut driver) = fixture();
        let mut recovery = Reconciler::default();
        driver.device.busid = "3-8".into();
        recovery.tick(&config, Duration::ZERO, &mut driver);
        driver.present = false;
        recovery.tick(&config, Duration::from_secs(3), &mut driver);
        recovery.tick(&config, Duration::from_secs(6), &mut driver);
        assert_eq!(driver.detached, vec![7]);
        driver.present = true;
        driver.device.path.push_str("-different-device");
        driver.ports.push(Port {
            number: 99,
            server: config.server.clone(),
            busid: driver.device.busid.clone(),
        });
        recovery.tick(&config, Duration::from_secs(9), &mut driver);
        recovery.tick(&config, Duration::from_secs(12), &mut driver);
        assert_eq!(driver.detached, vec![7]);
        assert_eq!(driver.attempts, 1);
    }
}
