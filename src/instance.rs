//! Machine-wide role locks. UI and background control have distinct lifetimes.
use std::io;
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError},
    System::Threading::{CreateMutexW, OpenMutexW},
};
pub struct Instance(usize);
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0 as _);
        }
    }
}
fn acquire_name(name: &str) -> io::Result<Option<Instance>> {
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    let error = unsafe { GetLastError() };
    if handle.is_null() {
        return if error == 5 {
            Ok(None)
        } else {
            Err(io::Error::from_raw_os_error(error as i32))
        };
    }
    if error == 183 {
        unsafe {
            CloseHandle(handle);
        }
        return Ok(None);
    }
    Ok(Some(Instance(handle as usize)))
}
pub fn acquire(role: &str, component: &str) -> io::Result<Option<Instance>> {
    acquire_name(&format!("Global\\USBtoNetworkBridge.{role}.{component}.1"))
}
pub fn running(role: &str, component: &str) -> io::Result<bool> {
    let name: Vec<u16> = format!("Global\\USBtoNetworkBridge.{role}.{component}.1")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe { OpenMutexW(0x00100000, 0, name.as_ptr()) };
    if !handle.is_null() {
        unsafe { CloseHandle(handle) };
        return Ok(true);
    }
    match unsafe { GetLastError() } {
        2 => Ok(false),
        5 => Ok(true),
        error => Err(io::Error::from_raw_os_error(error as i32)),
    }
}
pub fn stock_server_running() -> io::Result<bool> {
    let name: Vec<u16> = "Global\\usbipd-{A8256F62-728F-49B0-82BB-E5E48F83D28F}"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe { OpenMutexW(0x00100000, 0, name.as_ptr()) };
    if !handle.is_null() {
        unsafe {
            CloseHandle(handle);
        }
        return Ok(true);
    }
    match unsafe { GetLastError() } {
        2 => Ok(false),
        5 => Ok(true),
        error => Err(io::Error::from_raw_os_error(error as i32)),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_is_rejected_and_lock_is_released_when_owner_closes() {
        let name = format!("Global\\USBtoNetworkBridge.Test.{}", uuid::Uuid::new_v4());
        let first = acquire_name(&name).unwrap().unwrap();
        assert!(acquire_name(&name).unwrap().is_none());
        drop(first);
        assert!(acquire_name(&name).unwrap().is_some());
    }
}
