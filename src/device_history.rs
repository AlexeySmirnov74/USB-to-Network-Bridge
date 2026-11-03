//! Absence is established only by a successful physical USB enumeration.
//! Tombstones suppress stale driver/catalog records until the identity really returns.
use crate::{host_status::DeviceRow, settings};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io, path::Path};

pub const DAY_MS: u64 = 86_400_000;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Record {
    pub last_seen_unix_ms: u64,
    pub absent_since_unix_ms: Option<u64>,
    #[serde(default)]
    pub forgotten: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub devices: BTreeMap<String, Record>,
}
impl History {
    pub fn observe(&mut self, rows: &[DeviceRow], now: u64) {
        for row in rows {
            let entry = self
                .devices
                .entry(row.identity.to_ascii_lowercase())
                .or_default();
            if row.present {
                entry.last_seen_unix_ms = now;
                entry.absent_since_unix_ms = None;
                entry.forgotten = false;
            } else {
                let since = entry.absent_since_unix_ms.get_or_insert(now);
                if *since > now {
                    *since = now;
                } // clock correction must not trigger early deletion
            }
        }
    }
    pub fn visible(&self, row: &DeviceRow) -> bool {
        row.present
            || !self
                .devices
                .get(&row.identity.to_ascii_lowercase())
                .is_some_and(|r| r.forgotten)
    }
    pub fn expired(&self, rows: &[DeviceRow], now: u64, days: u32) -> Vec<String> {
        rows.iter()
            .filter(|row| !row.present)
            .filter(|row| {
                self.devices
                    .get(&row.identity.to_ascii_lowercase())
                    .is_some_and(|r| {
                        !r.forgotten
                            && r.absent_since_unix_ms.is_some_and(|since| {
                                now.saturating_sub(since) >= u64::from(days) * DAY_MS
                            })
                    })
            })
            .map(|r| r.identity.clone())
            .collect()
    }
}
pub fn file(path: &Path) -> std::path::PathBuf {
    path.with_extension("devices.local.json")
}
pub fn load(path: &Path) -> io::Result<History> {
    match settings::read(&file(path)) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(History::default()),
        Err(error) => Err(error),
    }
}
pub fn lock(path: &Path) -> io::Result<std::fs::File> {
    let start = std::time::Instant::now();
    loop {
        match settings::acquire_file_lock(&path.with_extension("devices.lock")) {
            Err(error)
                if matches!(error.raw_os_error(), Some(32 | 33))
                    && start.elapsed() < std::time::Duration::from_secs(3) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            result => return result,
        }
    }
}
pub fn observe(
    path: &Path,
    rows: &[DeviceRow],
    presence_verified: bool,
    now: u64,
    days: Option<u32>,
) -> io::Result<Vec<String>> {
    if !presence_verified {
        return Ok(Vec::new());
    }
    let _lock = lock(path)?;
    let mut history = load(path)?;
    history.observe(rows, now);
    settings::save(&file(path), &history)?;
    Ok(days
        .map(|days| history.expired(rows, now, days))
        .unwrap_or_default())
}

#[cfg(windows)]
pub fn forget(path: &Path, identity: &str) -> io::Result<()> {
    if !identity.starts_with("USB\\") {
        return Err(io::Error::other("Invalid USB identity"));
    }
    let _lock = lock(path)?;
    let host: settings::HostSettings = settings::read(path)?;
    host.validate()?;
    let physical = crate::host_status::physical_devices()?;
    if physical
        .iter()
        .any(|d| d.instance_id.eq_ignore_ascii_case(identity) && d.bus_id.is_some())
    {
        return Err(io::Error::other(
            "The device is connected to USB. Refresh the device list.",
        ));
    }
    let mut history = load(path)?;
    let inventory_path = host.runtime.join("devices.inventory");
    let mut inventory = match crate::inventory::load(&inventory_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        result => result?,
    };
    let state: serde_json::Value = settings::read(&host.runtime.join("backend-state.json"))?;
    let exported = state["ExportedBusIds"].as_array();
    if inventory.iter().any(|d| {
        d.path.eq_ignore_ascii_case(identity)
            && exported.is_some_and(|exports| exports.iter().any(|e| e.as_str() == Some(&d.busid)))
    }) {
        return Err(io::Error::other(
            "Unpublish the device before forgetting its descriptor cache",
        ));
    }
    inventory.retain(|d| !d.path.eq_ignore_ascii_case(identity));
    crate::inventory::save(&inventory_path, &inventory)?;
    let record = history
        .devices
        .entry(identity.to_ascii_lowercase())
        .or_default();
    record.forgotten = true;
    record
        .absent_since_unix_ms
        .get_or_insert(crate::recovery::timestamp());
    settings::save(&file(path), &history)?;
    let catalog_path = path.with_extension("catalog.local.json");
    if let Ok(mut catalog) = settings::read::<crate::management::Catalog>(&catalog_path) {
        catalog
            .devices
            .retain(|d| !d.identity.eq_ignore_ascii_case(identity));
        settings::save(&catalog_path, &catalog)?;
    }
    crate::diagnostics::event(
        "usb.device.forgotten",
        serde_json::json!({"identity":identity}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(present: bool) -> DeviceRow {
        DeviceRow {
            identity: "USB\\camera".into(),
            name: "Camera".into(),
            busid: "2-4".into(),
            state: String::new(),
            present,
            published: false,
            client: String::new(),
            tcp_port: String::new(),
            local_when_idle: false,
            kind: Default::default(),
        }
    }
    #[test]
    fn failed_enumeration_never_changes_persisted_history_or_expires_devices() {
        let folder =
            std::env::temp_dir().join(format!("usbbridge-history-{}", uuid::Uuid::new_v4()));
        let path = folder.join("host.json");
        assert!(
            observe(&path, &[row(false)], false, DAY_MS * 10, Some(1))
                .unwrap()
                .is_empty()
        );
        assert!(!file(&path).exists());
        observe(&path, &[row(false)], true, 100, Some(1)).unwrap();
        let before = std::fs::read(file(&path)).unwrap();
        assert!(
            observe(&path, &[row(false)], false, DAY_MS * 10, Some(1))
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read(file(&path)).unwrap(), before);
        assert!(
            observe(&path, &[row(false)], true, DAY_MS * 10, None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            observe(&path, &[row(false)], true, DAY_MS * 10, Some(1))
                .unwrap()
                .len(),
            1
        );
        std::fs::remove_dir_all(folder).unwrap();
    }
    #[test]
    fn absence_resets_on_return_in_another_port_and_survives_restart() {
        let mut h = History::default();
        h.observe(&[row(true)], 1);
        h.observe(&[row(false)], 100);
        assert!(h.expired(&[row(false)], DAY_MS + 99, 1).is_empty());
        assert_eq!(h.expired(&[row(false)], DAY_MS + 100, 1).len(), 1);
        let mut moved = row(true);
        moved.busid = "2-8".into();
        h.observe(&[moved], DAY_MS + 101);
        h.observe(&[row(false)], DAY_MS + 200);
        let h: History = serde_json::from_slice(&serde_json::to_vec(&h).unwrap()).unwrap();
        assert!(h.expired(&[row(false)], 2 * DAY_MS + 199, 1).is_empty());
    }
    #[test]
    fn forgetting_hides_stale_rows_and_return_revives_them() {
        let mut h = History::default();
        h.devices.insert(
            "usb\\camera".into(),
            Record {
                forgotten: true,
                ..Default::default()
            },
        );
        assert!(!h.visible(&row(false)));
        assert!(h.visible(&row(true)));
        h.observe(&[row(true)], 500);
        assert!(h.visible(&row(false)));
        assert!(h.expired(&[row(false)], 100 * DAY_MS, 1).is_empty());
    }
    #[test]
    fn clock_rollback_and_case_changes_do_not_delete_early() {
        let mut h = History::default();
        h.observe(&[row(false)], 2 * DAY_MS);
        h.observe(&[row(false)], 100);
        assert!(h.expired(&[row(false)], DAY_MS, 1).is_empty());
        let mut r = row(true);
        r.identity.make_ascii_uppercase();
        h.observe(&[r], 101);
        assert_eq!(h.devices.len(), 1);
        assert!(h.expired(&[row(false)], 100 * DAY_MS, 1).is_empty());
    }
}
