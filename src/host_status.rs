//! Desktop inventory and attach/detach telemetry, never per-transfer accounting.
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, net::SocketAddr};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Connection {
    #[serde(default)]
    pub identity: String,
    pub busid: String,
    pub client: SocketAddr,
    pub server: SocketAddr,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sessions {
    pub process_id: u32,
    pub connections: Vec<Connection>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PhysicalDevice {
    pub bus_id: Option<String>,
    pub description: String,
    pub instance_id: String,
    #[serde(rename = "ClientIPAddress")]
    pub client_ip_address: Option<std::net::IpAddr>,
    pub persisted_guid: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRow {
    #[serde(default)]
    pub kind: crate::device_kind::DeviceKind,
    pub identity: String,
    pub name: String,
    pub busid: String,
    pub state: String,
    pub present: bool,
    pub published: bool,
    pub client: String,
    pub tcp_port: String,
    #[serde(default)]
    pub local_when_idle: bool,
}
pub fn device_rows(
    physical: &[PhysicalDevice],
    inventory: &[crate::wire::Device],
    exports: &HashSet<String>,
    sessions: Option<&Sessions>,
    native: bool,
    running: bool,
    tcp_port: u16,
) -> Vec<DeviceRow> {
    let mut rows = Vec::new();
    for device in physical {
        let present = device.bus_id.is_some();
        let busid = device.bus_id.as_deref().unwrap_or("—");
        let published = if native {
            inventory
                .iter()
                .any(|d| d.path == device.instance_id && exports.contains(&d.busid))
        } else {
            device.persisted_guid.is_some()
        };
        let connection = sessions.and_then(|s| {
            s.connections.iter().find(|c| {
                if c.identity.is_empty() {
                    c.busid == busid
                } else {
                    c.identity.eq_ignore_ascii_case(&device.instance_id)
                }
            })
        });
        let client = if running && present {
            if native {
                connection.map(|c| c.client.to_string()).unwrap_or_default()
            } else {
                device
                    .client_ip_address
                    .map(|ip| ip.to_string())
                    .unwrap_or_default()
            }
        } else {
            String::new()
        };
        rows.push(DeviceRow {
            kind: crate::device_kind::DeviceKind::classify(
                &device.description,
                inventory
                    .iter()
                    .find(|d| d.path.eq_ignore_ascii_case(&device.instance_id)),
            ),
            local_when_idle: false,
            identity: device.instance_id.clone(),
            name: device.description.clone(),
            busid: busid.into(),
            present,
            published,
            state: if !present {
                "Не подключено к USB"
            } else if !published {
                "Не опубликовано"
            } else if !running {
                "Сервер выключен"
            } else if !client.is_empty() {
                "Используется клиентом"
            } else {
                "Доступно"
            }
            .into(),
            client: if client.is_empty() {
                "—".into()
            } else {
                client
            },
            tcp_port: if published {
                tcp_port.to_string()
            } else {
                "—".into()
            },
        });
    }
    for device in inventory.iter().filter(|d| exports.contains(&d.busid)) {
        if rows.iter().all(|r| r.identity != device.path) {
            rows.push(DeviceRow {
                kind: crate::device_kind::DeviceKind::classify(
                    &crate::catalog::device_name(device),
                    Some(device),
                ),
                local_when_idle: false,
                identity: device.path.clone(),
                name: crate::catalog::device_name(device),
                busid: device.busid.clone(),
                state: "Не подключено к USB".into(),
                present: false,
                published: true,
                client: "—".into(),
                tcp_port: tcp_port.to_string(),
            });
        }
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name).then(a.identity.cmp(&b.identity)));
    rows
}

#[cfg(windows)]
#[derive(Clone)]
pub struct Dashboard {
    pub presence_verified: bool,
    pub enabled: bool,
    pub running: bool,
    pub backend: String,
    pub tcp_port: u16,
    pub discovery_port: u16,
    pub rows: Vec<DeviceRow>,
    pub warning: Option<String>,
}
#[cfg(windows)]
pub fn collect(path: &std::path::Path) -> std::io::Result<Dashboard> {
    use crate::{
        agent,
        settings::{self, HostSettings},
    };
    use std::time::Duration;
    let host: HostSettings = settings::read(path)?;
    host.validate()?;
    let state: serde_json::Value = settings::read(&host.runtime.join("backend-state.json"))?;
    let backend = state["DesiredBackend"]
        .as_str()
        .or_else(|| state["Backend"].as_str())
        .unwrap_or("Native");
    let native = backend == "Native";
    let tcp_port = settings::endpoint(&host.probe_address)?.1;
    let process_running = if native {
        agent::native_alive(&host, &state)
    } else {
        crate::instance::stock_server_running()?
    };
    let running = process_running
        && crate::client::list_timeout(&host.probe_address, Duration::from_secs(2)).is_ok();
    let sessions = if native && running {
        settings::read::<Sessions>(&host.runtime.join("native-sessions.local.json"))
            .ok()
            .filter(|s| state["NativePid"].as_u64() == Some(s.process_id as u64))
    } else {
        None
    };
    let inventory =
        crate::inventory::load(&host.runtime.join("devices.inventory")).unwrap_or_default();
    let exports: HashSet<String> = state["ExportedBusIds"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|b| b.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_else(|| inventory.iter().map(|d| d.busid.clone()).collect());
    let physical = physical_devices();
    let presence_verified = physical.is_ok();
    let mut warning = physical
        .as_ref()
        .err()
        .map(|e| format!("Не удалось обновить список USB: {e}"));
    if !running
        && host.enabled
        && let Ok(report) = settings::read::<serde_json::Value>(&agent::status_path(path))
        && let Some(error) = report["error"].as_str()
    {
        warning = Some(error.into());
    }
    let mut rows = device_rows(
        &physical.unwrap_or_default(),
        &inventory,
        &exports,
        sessions.as_ref(),
        native,
        running,
        tcp_port,
    );
    if let Ok(previous) =
        settings::read::<crate::management::Catalog>(&path.with_extension("catalog.local.json"))
    {
        if !presence_verified {
            rows = previous.devices.clone();
        }
        for old in previous.devices {
            if rows.len() >= 1024 {
                break;
            }
            if !rows
                .iter()
                .any(|r| r.identity.eq_ignore_ascii_case(&old.identity))
            {
                let mut missing = old;
                missing.present = false;
                missing.client = "—".into();
                missing.state = "Не подключено к USB".into();
                rows.push(missing);
            }
        }
    }
    // A failed history read is an error, not permission to resurrect removed devices.
    let history = crate::device_history::load(path)?;
    rows.retain(|r| history.visible(r));
    rows.sort_by(|a, b| a.name.cmp(&b.name).then(a.identity.cmp(&b.identity)));
    let local: Vec<String> = state["LocalWhenIdle"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    for row in &mut rows {
        row.local_when_idle = local
            .iter()
            .any(|id| id.eq_ignore_ascii_case(&row.identity));
    }
    Ok(Dashboard {
        presence_verified,
        enabled: host.enabled,
        running,
        backend: backend.into(),
        tcp_port,
        discovery_port: host.discovery_port,
        rows,
        warning,
    })
}

#[cfg(windows)]
pub fn physical_devices() -> std::io::Result<Vec<PhysicalDevice>> {
    use std::{io, os::windows::process::CommandExt, process::Command};
    let program = std::env::var_os("ProgramFiles")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Program Files".into())
        .join("usbipd-win")
        .join("usbipd.exe");
    (|| -> io::Result<Vec<PhysicalDevice>> {
        #[derive(Deserialize)]
        struct Inventory {
            #[serde(rename = "Devices")]
            devices: Vec<PhysicalDevice>,
        }
        let output = Command::new(program)
            .arg("state")
            .creation_flags(0x08000000)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(serde_json::from_slice::<Inventory>(&output.stdout)
            .map_err(io::Error::other)?
            .devices)
    })()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn camera() -> PhysicalDevice {
        serde_json::from_str(r#"{"BusId":"1-2","Description":"Test camera","InstanceId":"USB\\VID_1234&PID_5678\\EXAMPLE","ClientIPAddress":"192.0.2.10","PersistedGuid":"shared"}"#).unwrap()
    }
    #[test]
    fn sessions_are_visible_only_for_live_present_devices_and_current_backend() {
        let physical = vec![camera()];
        let mut device = crate::backend::mock_device();
        device.path = physical[0].instance_id.clone();
        device.busid = "1-2".into();
        let exports = HashSet::from([device.busid.clone()]);
        let sessions = Sessions {
            process_id: 1,
            connections: vec![Connection {
                identity: physical[0].instance_id.clone(),
                busid: "1-2".into(),
                client: "192.0.2.20:50001".parse().unwrap(),
                server: "192.0.2.1:4000".parse().unwrap(),
            }],
        };
        let row = &device_rows(
            &physical,
            &[device.clone()],
            &exports,
            Some(&sessions),
            true,
            true,
            4000,
        )[0];
        assert_eq!(row.client, "192.0.2.20:50001");
        assert_eq!(row.tcp_port, "4000");
        assert_eq!(row.state, "Используется клиентом");
        let row = &device_rows(
            &physical,
            &[device.clone()],
            &exports,
            Some(&sessions),
            true,
            false,
            4000,
        )[0];
        assert_eq!(row.client, "—");
        assert_eq!(row.state, "Сервер выключен");
        let row = &device_rows(
            &physical,
            &[device.clone()],
            &exports,
            Some(&sessions),
            false,
            true,
            4000,
        )[0];
        assert_eq!(row.client, "192.0.2.10");
        let row = &device_rows(&[], &[device], &exports, Some(&sessions), true, true, 4000)[0];
        assert_eq!(row.state, "Не подключено к USB");
        assert_eq!(row.client, "—");
    }
    #[test]
    fn local_unshared_devices_do_not_appear_available_to_clients() {
        let rows = device_rows(&[camera()], &[], &HashSet::new(), None, true, true, 3240);
        assert_eq!(rows[0].state, "Не опубликовано");
        assert_eq!(rows[0].tcp_port, "—");
    }
}
