//! Durable control-plane settings. They never sit in the USB data path.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub fn read<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let file = File::open(path)?;
    let mut data = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut data)?;
    if data.len() > 1024 * 1024 {
        return Err(crate::wire::invalid("settings file exceeds 1 MiB"));
    }
    let data = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&data);
    serde_json::from_slice(data).map_err(io::Error::other)
}
pub fn save<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut data = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    data.push(b'\n');
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    struct Temporary(PathBuf);
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let cleanup = Temporary(temporary.clone());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&data)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    drop(cleanup);
    Ok(())
}
pub fn default_client_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("USBBridge")
        .join("client.json")
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub identity: String,
    pub vendor: u16,
    pub product: u16,
    pub busid: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub low_latency: bool,
}
fn yes() -> bool {
    true
}
fn poll() -> u64 {
    3
}
fn retry_max() -> u64 {
    30
}
fn port() -> u16 {
    crate::discovery::PORT
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientSettings {
    pub version: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
    pub server: String,
    #[serde(default)]
    pub server_name: String,
    #[serde(default)]
    pub proxy: Option<crate::proxy::Settings>,
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default = "yes")]
    pub discovery: bool,
    #[serde(default = "port")]
    pub discovery_port: u16,
    #[serde(default = "poll")]
    pub poll_seconds: u64,
    #[serde(default = "retry_max")]
    pub retry_max_seconds: u64,
    #[serde(default)]
    pub devices: Vec<Target>,
    /// Other saved servers; the legacy top-level fields hold the selected one.
    #[serde(default)]
    pub servers: Vec<SavedServer>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedServer {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
    pub server: String,
    #[serde(default)]
    pub proxy: Option<crate::proxy::Settings>,
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub devices: Vec<Target>,
}
impl SavedServer {
    pub fn key(&self) -> String {
        self.server_id
            .as_deref()
            .unwrap_or(&self.server)
            .to_ascii_lowercase()
    }
    pub fn matches(&self, other: &Self) -> bool {
        match (&self.server_id, &other.server_id) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => self.server.eq_ignore_ascii_case(&other.server),
        }
    }
}
impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            version: 1,
            paused: false,
            server: String::new(),
            server_name: String::new(),
            proxy: None,
            server_id: None,
            discovery: true,
            discovery_port: port(),
            poll_seconds: poll(),
            retry_max_seconds: retry_max(),
            devices: Vec::new(),
            servers: Vec::new(),
        }
    }
}
impl ClientSettings {
    pub fn connections(&self) -> Vec<SavedServer> {
        let mut result = Vec::new();
        if !self.server.is_empty() {
            result.push(SavedServer {
                paused: self.paused,
                server: self.server.clone(),
                proxy: self.proxy.clone(),
                server_id: self.server_id.clone(),
                name: self.server_name.clone(),
                devices: self.devices.clone(),
            });
        }
        result.extend(self.servers.clone());
        result
    }
    pub fn connection_config(&self, connection: &SavedServer) -> Self {
        Self {
            paused: connection.paused,
            server: connection.server.clone(),
            proxy: connection.proxy.clone(),
            server_id: connection.server_id.clone(),
            server_name: connection.name.clone(),
            devices: connection.devices.clone(),
            servers: Vec::new(),
            ..self.clone()
        }
    }
    /// Select a server without discarding another server's remembered USB targets.
    pub fn select_server(&mut self, mut selected: SavedServer) {
        let mut connections = self.connections();
        if let Some(previous) = connections.iter().find(|s| s.matches(&selected)) {
            selected.paused = previous.paused;
            selected.devices = previous.devices.clone();
            if selected.proxy.is_none() {
                selected.proxy = previous.proxy.clone();
            }
            if selected.server_id.is_none() {
                selected.server_id = previous.server_id.clone();
            }
            if selected.name.is_empty() {
                selected.name = previous.name.clone();
            }
        }
        connections.retain(|s| !s.matches(&selected));
        self.server = selected.server;
        self.paused = selected.paused;
        self.proxy = selected.proxy;
        self.server_id = selected.server_id;
        self.server_name = selected.name;
        self.devices = selected.devices;
        self.servers = connections;
    }
    pub fn update_connection(&mut self, key: &str, change: impl FnOnce(&mut SavedServer)) -> bool {
        let mut connections = self.connections();
        let Some(index) = connections
            .iter()
            .position(|s| s.key().eq_ignore_ascii_case(key))
        else {
            return false;
        };
        change(&mut connections[index]);
        if index == 0 && !self.server.is_empty() {
            let primary = connections.remove(0);
            self.server = primary.server;
            self.paused = primary.paused;
            self.proxy = primary.proxy;
            self.server_id = primary.server_id;
            self.server_name = primary.name;
            self.devices = primary.devices;
        } else if !self.server.is_empty() {
            connections.remove(0);
        }
        self.servers = connections;
        true
    }
    pub fn remove_server(&mut self, key: &str) {
        let mut connections = self.connections();
        connections.retain(|s| s.key() != key);
        self.server.clear();
        self.paused = false;
        self.proxy = None;
        self.server_id = None;
        self.server_name.clear();
        self.devices.clear();
        self.servers.clear();
        if !connections.is_empty() {
            let first = connections.remove(0);
            self.server = first.server;
            self.paused = first.paused;
            self.proxy = first.proxy;
            self.server_id = first.server_id;
            self.server_name = first.name;
            self.devices = first.devices;
            self.servers = connections;
        }
    }
    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || !(1..=60).contains(&self.poll_seconds)
            || !(2..=300).contains(&self.retry_max_seconds)
            || self.discovery_port == 0
        {
            return Err(crate::wire::invalid(
                "invalid client settings version or retry interval",
            ));
        }
        if !self.server.is_empty() {
            endpoint(&self.server)?;
        }
        if let Some(id) = &self.server_id {
            uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
        }
        let mut keys = std::collections::HashSet::new();
        let connections = self.connections();
        for connection in &connections {
            endpoint(&connection.server)?;
            if let Some(proxy) = &connection.proxy {
                proxy.validate()?;
            }
            if connection.name.len() > 128 || !keys.insert(connection.key()) {
                return Err(crate::wire::invalid("duplicate or invalid saved server"));
            }
            if let Some(id) = &connection.server_id {
                uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
            }
        }
        for devices in std::iter::once(&self.devices).chain(self.servers.iter().map(|s| &s.devices))
        {
            let mut identities = std::collections::HashSet::new();
            for device in devices {
                if device.identity.is_empty()
                    || device.identity.len() > 255
                    || device.busid.len() >= 32
                    || !identities.insert(device.identity.to_ascii_lowercase())
                {
                    return Err(crate::wire::invalid(
                        "device identities must be non-empty and unique",
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn remember(&mut self, device: &crate::wire::Device, low_latency: bool) {
        self.paused = false;
        let target = Target {
            identity: device.path.clone(),
            vendor: device.vendor,
            product: device.product,
            busid: device.busid.clone(),
            enabled: true,
            low_latency,
        };
        if let Some(previous) = self
            .devices
            .iter_mut()
            .find(|d| d.identity.eq_ignore_ascii_case(&target.identity))
        {
            *previous = target;
        } else {
            self.devices.push(target);
        }
    }
}
pub fn endpoint(value: &str) -> io::Result<(String, u16)> {
    let (host, port) = if let Some(value) = value.strip_prefix('[') {
        value
            .split_once("]:")
            .ok_or_else(|| crate::wire::invalid("use [IPv6]:port"))?
    } else {
        let parts = value
            .rsplit_once(':')
            .ok_or_else(|| crate::wire::invalid("use HOST:PORT"))?;
        if parts.0.contains(':') {
            return Err(crate::wire::invalid("use [IPv6]:port"));
        }
        parts
    };
    let port: u16 = port.parse().map_err(io::Error::other)?;
    if host.is_empty() || host.len() > 1024 || port == 0 || host.chars().any(char::is_whitespace) {
        return Err(crate::wire::invalid("invalid server endpoint"));
    }
    Ok((host.to_string(), port))
}
pub fn normalize_endpoint(value: &str, default_port: u16) -> io::Result<String> {
    let value = value.trim();
    let address = if let Ok(ip) = value.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        std::net::SocketAddr::new(ip, default_port).to_string()
    } else if value.contains(':') {
        value.to_string()
    } else {
        format!("{value}:{default_port}")
    };
    endpoint(&address)?;
    Ok(address)
}
pub fn edit_client<T>(
    path: &Path,
    change: impl FnOnce(&mut ClientSettings) -> io::Result<T>,
) -> io::Result<T> {
    let due = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let _lock = loop {
        match acquire_file_lock(&path.with_extension("edit.lock")) {
            Ok(file) => break file,
            Err(error) if error.raw_os_error() == Some(32) && std::time::Instant::now() < due => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            Err(error) => return Err(error),
        }
    };
    let mut config = match read::<ClientSettings>(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => ClientSettings::default(),
        Err(e) => return Err(e),
    };
    config.validate()?;
    let result = change(&mut config)?;
    config.validate()?;
    save(path, &config)?;
    Ok(result)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSettings {
    pub version: u32,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub server_id: String,
    pub name: String,
    pub runtime: PathBuf,
    pub profile: PathBuf,
    pub allowed_ips: Vec<std::net::IpAddr>,
    #[serde(default = "probe_address")]
    pub probe_address: String,
    #[serde(default = "port")]
    pub discovery_port: u16,
    #[serde(default = "yes")]
    pub auto_remove_unplugged: bool,
    #[serde(default = "retention_days")]
    pub unplugged_retention_days: u32,
}
fn retention_days() -> u32 {
    1
}
fn probe_address() -> String {
    "127.0.0.1:3240".into()
}
impl HostSettings {
    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || self.name.is_empty()
            || self.name.len() > 128
            || self.discovery_port == 0
            || !(1..=36500).contains(&self.unplugged_retention_days)
            || self.allowed_ips.is_empty()
            || !self.runtime.is_absolute()
            || !self.profile.is_absolute()
        {
            return Err(crate::wire::invalid("invalid host settings"));
        }
        uuid::Uuid::parse_str(&self.server_id).map_err(io::Error::other)?;
        if endpoint(&self.probe_address)?.1 == self.discovery_port {
            return Err(crate::wire::invalid(
                "USB data and status/discovery ports must differ",
            ));
        }
        Ok(())
    }
}
pub fn create_host_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn acquire_file_lock(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options.open(path)
}
pub fn acquire_operation_lock(path: &Path, server_key: &str, identity: &str) -> io::Result<File> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    server_key.to_ascii_lowercase().hash(&mut hash);
    identity.to_ascii_lowercase().hash(&mut hash);
    acquire_file_lock(&path.with_extension(format!("operation-{:016x}.lock", hash.finish())))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_host_is_enabled_and_a_saved_pause_survives_roundtrip() {
        let mut value = serde_json::json!({"version":1,"server_id":uuid::Uuid::new_v4().to_string(),"name":"example","runtime":"C:\\example","profile":"C:\\example\\server.psd1","allowed_ips":["192.0.2.1"]});
        let legacy: HostSettings = serde_json::from_value(value.clone()).unwrap();
        assert!(legacy.auto_remove_unplugged);
        assert_eq!(legacy.unplugged_retention_days, 1);
        assert!(
            serde_json::from_value::<HostSettings>(value.clone())
                .unwrap()
                .enabled
        );
        value["enabled"] = false.into();
        let paused: HostSettings = serde_json::from_value(value).unwrap();
        assert!(
            !serde_json::from_slice::<HostSettings>(&serde_json::to_vec(&paused).unwrap())
                .unwrap()
                .enabled
        );
    }
    #[test]
    fn server_pause_survives_selection_restart_and_resumes_without_enabling_other_devices() {
        let mut config = ClientSettings::default();
        let server = SavedServer {
            paused: false,
            server: "192.0.2.1:3240".into(),
            proxy: None,
            server_id: None,
            name: "first".into(),
            devices: Vec::new(),
        };
        config.select_server(server.clone());
        config.remember(&crate::backend::mock_device(), false);
        let mut disabled = config.devices[0].clone();
        disabled.identity = "disabled-mouse".into();
        disabled.busid = "2-3".into();
        disabled.enabled = false;
        config.devices.push(disabled);
        let targets = serde_json::to_value(&config.devices).unwrap();
        config.update_connection(&server.key(), |s| s.paused = true);
        let second = SavedServer {
            server: "192.0.2.2:3240".into(),
            name: "second".into(),
            ..server.clone()
        };
        config.select_server(second.clone());
        assert!(!config.paused);
        assert!(
            config
                .connections()
                .iter()
                .find(|s| s.matches(&server))
                .unwrap()
                .paused
        );
        let mut restored: ClientSettings =
            serde_json::from_slice(&serde_json::to_vec(&config).unwrap()).unwrap();
        restored.select_server(server.clone());
        assert!(restored.paused);
        restored.update_connection(&server.key(), |s| s.paused = false);
        assert!(!restored.paused);
        assert_eq!(serde_json::to_value(&restored.devices).unwrap(), targets);
        let legacy: ClientSettings =
            serde_json::from_value(serde_json::json!({"version":1,"server":"192.0.2.1:3240"}))
                .unwrap();
        assert!(!legacy.paused);
        assert!(
            !serde_json::to_value(&legacy)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("paused")
        );
    }
    #[test]
    fn switching_servers_keeps_independent_device_choices_and_ports() {
        let mut settings = ClientSettings::default();
        let first = SavedServer {
            paused: false,
            proxy: None,
            server: "192.0.2.1:4100".into(),
            server_id: Some(uuid::Uuid::new_v4().to_string()),
            name: "same name".into(),
            devices: Vec::new(),
        };
        let second = SavedServer {
            paused: false,
            proxy: None,
            server: "192.0.2.2:5200".into(),
            server_id: Some(uuid::Uuid::new_v4().to_string()),
            name: "same name".into(),
            devices: Vec::new(),
        };
        settings.select_server(first.clone());
        settings.remember(&crate::backend::mock_device(), false);
        settings.select_server(second.clone());
        settings.remember(&crate::backend::mock_device(), true);
        settings.select_server(first.clone());
        assert_eq!(settings.connections().len(), 2);
        assert_eq!(settings.devices.len(), 1);
        assert!(!settings.devices[0].low_latency);
        settings.update_connection(&second.key(), |s| {
            s.server = "192.0.2.2:5300".into();
            s.devices[0].enabled = false;
        });
        settings.select_server(SavedServer {
            paused: false,
            proxy: None,
            server: "192.0.2.1:4200".into(),
            ..first
        });
        settings.validate().unwrap();
        let restored: ClientSettings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(restored.server, "192.0.2.1:4200");
        assert!(restored.devices[0].enabled);
        assert_eq!(restored.servers[0].server, "192.0.2.2:5300");
        assert!(!restored.servers[0].devices[0].enabled);
        let primary = restored.connections()[0].key();
        settings.remove_server(&primary);
        assert_eq!(settings.connections().len(), 1);
        assert_eq!(settings.server_id, second.server_id);
    }
    #[test]
    fn legacy_config_and_manual_server_identity_upgrade_keep_devices() {
        let mut config: ClientSettings =
            serde_json::from_str(r#"{"version":1,"server":"192.0.2.10:3240","devices":[]}"#)
                .unwrap();
        config.remember(&crate::backend::mock_device(), false);
        config.select_server(SavedServer {
            paused: false,
            proxy: None,
            server: config.server.clone(),
            server_id: Some(uuid::Uuid::new_v4().to_string()),
            name: "host".into(),
            devices: Vec::new(),
        });
        config.validate().unwrap();
        assert_eq!(config.connections().len(), 1);
        assert_eq!(config.devices.len(), 1);
        assert_eq!(
            normalize_endpoint("2001:db8::1", 4310).unwrap(),
            "[2001:db8::1]:4310"
        );
        assert!(normalize_endpoint("host:0", 3240).is_err());
        assert!(normalize_endpoint("host:65536", 3240).is_err());
    }
    #[test]
    fn editing_a_manual_servers_port_keeps_one_profile_and_its_usb_targets() {
        let mut config = ClientSettings {
            server: "192.0.2.5:3240".into(),
            ..Default::default()
        };
        config.remember(&crate::backend::mock_device(), false);
        let old = config.connections()[0].key();
        config.update_connection(&old, |s| s.server = "192.0.2.5:5500".into());
        config.select_server(SavedServer {
            paused: false,
            proxy: None,
            server: "192.0.2.5:5500".into(),
            server_id: None,
            name: String::new(),
            devices: Vec::new(),
        });
        config.validate().unwrap();
        assert_eq!(config.connections().len(), 1);
        assert_eq!(config.devices.len(), 1);
    }
    #[test]
    fn proxy_is_independent_per_server_and_survives_selection_and_identity_upgrade() {
        let mut config = ClientSettings {
            server: "192.0.2.1:3240".into(),
            proxy: Some(crate::proxy::Settings {
                kind: crate::proxy::Kind::Socks5,
                address: "proxy.invalid:1080".into(),
                credential_file: None,
            }),
            ..Default::default()
        };
        config.remember(&crate::backend::mock_device(), false);
        let mut first = config.connections()[0].clone();
        first.server_id = Some(uuid::Uuid::new_v4().to_string());
        first.proxy = None;
        config.select_server(first.clone());
        assert!(config.proxy.is_some());
        config.select_server(SavedServer {
            paused: false,
            server: "192.0.2.2:3240".into(),
            proxy: None,
            server_id: None,
            name: "second".into(),
            devices: Vec::new(),
        });
        assert!(config.proxy.is_none());
        config.select_server(first.clone());
        assert_eq!(config.proxy.as_ref().unwrap().address, "proxy.invalid:1080");
        assert_eq!(config.devices.len(), 1);
        config.update_connection(&first.key(), |s| s.proxy = None);
        let restored: ClientSettings =
            serde_json::from_slice(&serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(restored.proxy.is_none());
        assert_eq!(restored.connections().len(), 2);
    }
    #[test]
    fn host_rejects_overlapping_data_and_catalog_listeners() {
        let mut host = HostSettings {
            auto_remove_unplugged: true,
            unplugged_retention_days: 1,
            version: 1,
            enabled: true,
            server_id: uuid::Uuid::new_v4().to_string(),
            name: "host".into(),
            runtime: std::env::temp_dir(),
            profile: std::env::temp_dir().join("server.psd1"),
            allowed_ips: vec!["192.0.2.1".parse().unwrap()],
            probe_address: "127.0.0.1:3240".into(),
            discovery_port: 3240,
        };
        assert!(host.validate().is_err());
        host.discovery_port = 3241;
        assert!(host.validate().is_ok());
        for days in [0, 36501, u32::MAX] {
            host.unplugged_retention_days = days;
            assert!(host.validate().is_err());
        }
    }
    #[test]
    fn persistent_selection_is_identity_based_and_atomic() {
        let folder =
            std::env::temp_dir().join(format!("usbbridge-settings-{}", uuid::Uuid::new_v4()));
        let path = folder.join("client.json");
        let mut settings = ClientSettings::default();
        let mut device = crate::backend::mock_device();
        settings.remember(&device, false);
        device.busid = "2-9".into();
        settings.remember(&device, true);
        settings.validate().unwrap();
        save(&path, &settings).unwrap();
        settings.server = "127.0.0.1:3240".into();
        save(&path, &settings).unwrap();
        let restored: ClientSettings = read(&path).unwrap();
        assert_eq!(restored.devices.len(), 1);
        assert_eq!(restored.devices[0].busid, "2-9");
        assert!(restored.devices[0].low_latency);
        assert_eq!(restored.server, "127.0.0.1:3240");
        fs::remove_file(path).unwrap();
        fs::remove_dir(folder).unwrap();
    }
    #[test]
    fn reject_ambiguous_or_invalid_configuration() {
        let mut settings = ClientSettings::default();
        settings.remember(&crate::backend::mock_device(), false);
        settings.devices.push(settings.devices[0].clone());
        assert!(settings.validate().is_err());
        assert!(endpoint("::1:3240").is_err());
        assert_eq!(endpoint("[::1]:3240").unwrap(), ("::1".into(), 3240));
        assert!(
            serde_json::from_str::<ClientSettings>(r#"{"version":1,"server":"","deivces":[]}"#)
                .is_err()
        );
    }
}
