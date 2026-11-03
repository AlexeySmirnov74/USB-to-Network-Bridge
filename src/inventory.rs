//! Explicit inventory for devices reserved for the native capture backend.
use crate::wire::{self, Device};
use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
};
const MAGIC: &[u8; 16] = b"USBBRIDGE-INV01\0";
pub fn save(path: &Path, devices: &[Device]) -> io::Result<()> {
    if devices.len() > 65535 {
        return Err(wire::invalid("too many inventory records"));
    }
    // Validate and encode before replacing a previously usable inventory.
    let mut bytes = Vec::new();
    bytes.write_all(MAGIC)?;
    bytes.write_all(&(devices.len() as u32).to_be_bytes())?;
    for device in devices {
        bytes.write_all(&device.encode()?)?;
        for interface in &device.interfaces {
            bytes.write_all(interface)?;
        }
    }
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(wire::invalid("inventory too large"));
    }
    read_inventory(&mut &bytes[..])?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}
/// Cache slots are stable per identity; physical ports can change or be reused.
pub fn merge(mut existing: Vec<Device>, incoming: Vec<Device>) -> Vec<Device> {
    for mut device in incoming {
        if let Some(old) = existing
            .iter_mut()
            .find(|d| d.path.eq_ignore_ascii_case(&device.path))
        {
            device.busid = old.busid.clone();
            *old = device;
        } else {
            if existing.iter().any(|d| d.busid == device.busid) {
                device.busid = format!("cache-{}", uuid::Uuid::new_v4().simple());
                device.busid.truncate(31);
            }
            existing.push(device);
        }
    }
    existing
}
pub fn load(path: &Path) -> io::Result<Vec<Device>> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() > 32 * 1024 * 1024 {
        return Err(wire::invalid("inventory too large"));
    }
    read_inventory(&mut file)
}
fn read_inventory(file: &mut impl Read) -> io::Result<Vec<Device>> {
    let mut magic = [0; 16];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(wire::invalid("unrecognized inventory format"));
    }
    let mut count = [0; 4];
    file.read_exact(&mut count)?;
    let count = wire::u32be(&count);
    if count > 65535 {
        return Err(wire::invalid("too many inventory records"));
    }
    let mut devices = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut identities = std::collections::HashSet::new();
    for _ in 0..count {
        let mut bytes = [0; 312];
        file.read_exact(&mut bytes)?;
        let mut device = Device::decode(&bytes)?;
        for interface in &mut device.interfaces {
            file.read_exact(interface)?;
        }
        if !seen.insert(device.busid.clone())
            || !identities.insert(device.path.to_ascii_lowercase())
            || !device.path.starts_with("USB\\")
        {
            return Err(wire::invalid(
                "inventory must contain unique Windows USB instance IDs",
            ));
        }
        devices.push(device);
    }
    let mut trailing = [0; 1];
    if file.read(&mut trailing)? != 0 {
        return Err(wire::invalid("trailing inventory data"));
    }
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_preserves_identity_slots_when_a_physical_port_is_reused() {
        let mut original = crate::backend::mock_device();
        original.path = "USB\\original".into();
        original.busid = "1-2".into();
        let mut moved = original.clone();
        moved.busid = "1-3".into();
        let mut other = original.clone();
        other.path = "USB\\other".into();
        let merged = merge(vec![original], vec![moved, other]);
        assert_eq!(merged[0].busid, "1-2");
        assert_ne!(merged[1].busid, "1-2");
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&2u32.to_be_bytes());
        for d in merged {
            bytes.extend_from_slice(&d.encode().unwrap());
            for interface in d.interfaces {
                bytes.extend_from_slice(&interface);
            }
        }
        assert_eq!(read_inventory(&mut &bytes[..]).unwrap().len(), 2);
    }
    fn record() -> Vec<u8> {
        let mut device = crate::backend::mock_device();
        device.path = "USB\\VID_1234&PID_5678\\EXAMPLE".into();
        device.busid = "1-2".into();
        let mut data = MAGIC.to_vec();
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&device.encode().unwrap());
        for interface in device.interfaces {
            data.extend_from_slice(&interface);
        }
        data
    }
    #[test]
    fn inventory_roundtrip_and_truncation() {
        let data = record();
        let devices = read_inventory(&mut &data[..]).unwrap();
        assert_eq!(devices[0].busid, "1-2");
        assert_eq!(devices[0].interfaces, vec![[0xff, 0, 0, 0]]);
        for length in 0..data.len() {
            assert!(read_inventory(&mut &data[..length]).is_err());
        }
    }
    #[test]
    fn reject_duplicate_foreign_and_trailing_records() {
        let mut data = record();
        data[19] = 2;
        let duplicate = data[20..].to_vec();
        data.extend_from_slice(&duplicate);
        assert!(read_inventory(&mut &data[..]).is_err());
        let mut data = record();
        data[20] = b'X';
        assert!(read_inventory(&mut &data[..]).is_err());
        let mut data = record();
        data.push(0);
        assert!(read_inventory(&mut &data[..]).is_err());
    }
}
