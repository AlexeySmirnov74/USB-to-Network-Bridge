//! Atomic, identity-scoped live publication; never touches unrelated sessions.
use crate::{
    settings,
    wire::{self, Device},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub revision: String,
    pub devices: Vec<Device>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_when_idle: Vec<String>,
}
impl Snapshot {
    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1 || self.devices.len() > 65535 || self.local_when_idle.len() > 65535 {
            return Err(wire::invalid("invalid publication snapshot"));
        }
        uuid::Uuid::parse_str(&self.revision).map_err(io::Error::other)?;
        let mut local = HashSet::new();
        for identity in &self.local_when_idle {
            if !identity.starts_with("USB\\")
                || identity.len() > 255
                || !local.insert(identity.to_ascii_lowercase())
            {
                return Err(wire::invalid("invalid local-use device policy"));
            }
        }
        let mut identities = HashSet::new();
        let mut locations = HashSet::new();
        for device in &self.devices {
            device.encode()?;
            if !device.path.starts_with("USB\\")
                || device.class == 9
                || !identities.insert(device.path.to_ascii_lowercase())
                || !locations.insert(device.busid.clone())
            {
                return Err(wire::invalid(
                    "publication requires unique USB identities and cached locations; hubs cannot be published",
                ));
            }
        }
        Ok(())
    }
    pub fn load(path: &Path) -> io::Result<Self> {
        let value: Self = settings::read(path)?;
        value.validate()?;
        Ok(value)
    }
}
struct Cached {
    snapshot: Snapshot,
    error: Option<String>,
}
pub struct Store {
    path: PathBuf,
    cached: Mutex<Cached>,
}
impl Store {
    pub fn load(path: &Path) -> io::Result<Self> {
        Ok(Self {
            path: path.into(),
            cached: Mutex::new(Cached {
                snapshot: Snapshot::load(path)?,
                error: None,
            }),
        })
    }
    pub fn devices(&self) -> Vec<Device> {
        self.snapshot().devices
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut cache = self.cached.lock().unwrap();
        match Snapshot::load(&self.path) {
            Ok(snapshot) => {
                if snapshot.revision != cache.snapshot.revision {
                    crate::diagnostics::event(
                        "publication.applied",
                        serde_json::json!({"revision":snapshot.revision,"devices":snapshot.devices.iter().map(|d|&d.path).collect::<Vec<_>>()}),
                    );
                }
                cache.snapshot = snapshot;
                cache.error = None;
            }
            Err(error) => {
                let message = error.to_string();
                if cache.error.as_deref() != Some(&message) {
                    crate::diagnostics::event(
                        "publication.read.failed",
                        serde_json::json!({"error":message,"retained_revision":cache.snapshot.revision}),
                    );
                    cache.error = Some(message);
                }
            }
        }
        // A missing/invalid replacement file is not evidence of USB removal.
        cache.snapshot.clone()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_use_defaults_off_and_remembers_unpublished_identities() {
        let text = serde_json::json!({"version":1,"revision":uuid::Uuid::new_v4().to_string(),"devices":[]});
        let mut snapshot: Snapshot = serde_json::from_value(text).unwrap();
        assert!(snapshot.local_when_idle.is_empty());
        snapshot
            .local_when_idle
            .push("USB\\VID_1234&PID_5678\\saved".into());
        snapshot.validate().unwrap();
        let restored: Snapshot =
            serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert_eq!(restored.local_when_idle, snapshot.local_when_idle);
        snapshot
            .local_when_idle
            .push(snapshot.local_when_idle[0].to_ascii_lowercase());
        assert!(snapshot.validate().is_err());
    }
    #[test]
    fn invalid_update_keeps_active_identities_and_empty_update_is_explicit() {
        let path = std::env::temp_dir().join(format!("publication-{}.json", uuid::Uuid::new_v4()));
        let mut device = crate::backend::mock_device();
        device.path = "USB\\VID_1209&PID_F001\\fixture".into();
        let mut snapshot = Snapshot {
            version: 1,
            revision: uuid::Uuid::new_v4().to_string(),
            devices: vec![device.clone()],
            local_when_idle: Vec::new(),
        };
        settings::save(&path, &snapshot).unwrap();
        let store = Store::load(&path).unwrap();
        std::fs::write(&path, b"{partial update").unwrap();
        assert_eq!(store.devices(), vec![device]);
        snapshot.devices.clear();
        snapshot.revision = uuid::Uuid::new_v4().to_string();
        settings::save(&path, &snapshot).unwrap();
        assert!(store.devices().is_empty());
        std::fs::remove_file(&path).unwrap();
        assert!(store.devices().is_empty());
    }
    #[test]
    fn duplicate_identities_and_hubs_are_rejected() {
        let mut device = crate::backend::mock_device();
        device.path = "USB\\fixture".into();
        let mut snapshot = Snapshot {
            version: 1,
            revision: uuid::Uuid::new_v4().to_string(),
            devices: vec![device.clone()],
            local_when_idle: Vec::new(),
        };
        assert!(snapshot.validate().is_ok());
        device.path = "USB\\FIXTURE".into();
        device.busid = "another".into();
        snapshot.devices.push(device);
        assert!(snapshot.validate().is_err());
        snapshot.devices.pop();
        snapshot.devices[0].class = 9;
        assert!(snapshot.validate().is_err());
    }
}
