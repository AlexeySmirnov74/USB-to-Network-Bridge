//! Server/device catalog shared by the desktop UI and its background refresh.
use crate::{
    discovery::FoundServer,
    settings::{ClientSettings, SavedServer},
    wire::Device,
};
#[derive(Clone, Debug)]
pub struct Server {
    pub saved: SavedServer,
    pub addresses: Vec<String>,
    pub devices: Vec<Device>,
    pub error: Option<String>,
    pub catalog: Option<crate::management::Catalog>,
}
pub fn merge(config: &ClientSettings, found: Vec<FoundServer>) -> Vec<Server> {
    let mut output: Vec<_> = config
        .connections()
        .into_iter()
        .map(|saved| Server {
            addresses: vec![saved.server.clone()],
            saved,
            devices: Vec::new(),
            error: None,
            catalog: None,
        })
        .collect();
    for discovered in crate::discovery::group_servers(found) {
        let saved = SavedServer {
            paused: false,
            proxy: None,
            server: discovered.address.clone(),
            server_id: Some(discovered.info.id),
            name: discovered.info.name,
            devices: Vec::new(),
        };
        if let Some(previous) = output.iter_mut().find(|s| {
            s.saved.matches(&saved)
                || (s.saved.server_id.is_none() && discovered.addresses.contains(&s.saved.server))
        }) {
            previous.saved.server_id = saved.server_id;
            previous.saved.name = saved.name;
            previous.addresses = if previous.saved.proxy.is_some() {
                vec![previous.saved.server.clone()]
            } else {
                discovered.addresses
            };
            previous.addresses.retain(|a| a != &previous.saved.server);
            previous.addresses.insert(0, previous.saved.server.clone());
        } else {
            output.push(Server {
                saved,
                addresses: discovered.addresses,
                devices: Vec::new(),
                error: None,
                catalog: None,
            });
        }
    }
    output
}
pub fn device_name(device: &Device) -> String {
    let video = device.interfaces.iter().any(|i| i[0] == 14) || device.class == 14;
    let storage = device.interfaces.iter().any(|i| i[0] == 8) || device.class == 8;
    let hid = device.interfaces.iter().any(|i| i[0] == 3) || device.class == 3;
    let kind = if video {
        "Камера"
    } else if storage {
        "Накопитель"
    } else if hid {
        "Устройство ввода"
    } else {
        "USB-устройство"
    };
    format!(
        "{kind} {:04x}:{:04x}  [{}]",
        device.vendor, device.product, device.busid
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_keeps_the_saved_route_and_offline_device_selection() {
        let id = uuid::Uuid::new_v4().to_string();
        let mut config = ClientSettings::default();
        config.select_server(SavedServer {
            paused: false,
            proxy: None,
            server: "26.0.0.1:4900".into(),
            server_id: Some(id.clone()),
            name: "old".into(),
            devices: Vec::new(),
        });
        config.remember(&crate::backend::mock_device(), false);
        let found = FoundServer {
            address: "192.168.1.1:4900".into(),
            addresses: Vec::new(),
            info: crate::discovery::ServerInfo {
                id,
                name: "host".into(),
                tcp_port: 4900,
                backend: "Native".into(),
            },
        };
        let catalog = merge(&config, vec![found]);
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].addresses[0], "26.0.0.1:4900");
        assert_eq!(catalog[0].saved.devices.len(), 1);
        assert_eq!(merge(&config, Vec::new()).len(), 1);
    }
}
