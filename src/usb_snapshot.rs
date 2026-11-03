//! Read descriptors through the parent Windows USB hub, without capture/reset.
use crate::{
    settings,
    wire::{self, Device},
};
use serde::Deserialize;
use std::{
    io,
    mem::size_of,
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Devices::{DeviceAndDriverInstallation::*, Usb::*},
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{CreateFileW, OPEN_EXISTING},
    System::IO::DeviceIoControl,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    identity: String,
    node_instance: String,
    busid: String,
}
struct Hub(HANDLE);
impl Drop for Hub {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn cm(result: u32) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "USB parent hub query: CONFIGRET {result}"
        )))
    }
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn parent_hub(instance: &str) -> io::Result<Hub> {
    let instance = wide(instance);
    let mut node = 0;
    let mut parent = 0;
    let mut len = 0;
    unsafe {
        cm(CM_Locate_DevNodeW(&mut node, instance.as_ptr(), 0))?;
        cm(CM_Get_Parent(&mut parent, node, 0))?;
        cm(CM_Get_Device_ID_Size(&mut len, parent, 0))?;
        let mut id = vec![0; len as usize + 1];
        cm(CM_Get_Device_IDW(
            parent,
            id.as_mut_ptr(),
            id.len() as u32,
            0,
        ))?;
        let mut size = 0;
        cm(CM_Get_Device_Interface_List_SizeW(
            &mut size,
            &GUID_DEVINTERFACE_USB_HUB,
            id.as_ptr(),
            0,
        ))?;
        let mut path = vec![0; size as usize];
        cm(CM_Get_Device_Interface_ListW(
            &GUID_DEVINTERFACE_USB_HUB,
            id.as_ptr(),
            path.as_mut_ptr(),
            size,
            0,
        ))?;
        if path.first().copied().unwrap_or(0) == 0 {
            return Err(wire::invalid("parent USB hub is not present"));
        }
        let handle = CreateFileW(
            path.as_ptr(),
            0xc0000000,
            3,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Hub(handle))
    }
}
fn query(hub: &Hub, code: u32, buffer: &mut [u8]) -> io::Result<usize> {
    let mut returned = 0;
    let ok = unsafe {
        DeviceIoControl(
            hub.0,
            code,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            &mut returned,
            null_mut(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(returned as usize)
    }
}
fn configuration(hub: &Hub, port: u32, length: u16) -> io::Result<Vec<u8>> {
    // USB_DESCRIPTOR_REQUEST: ConnectionIndex + USB_SETUP_PACKET + data.
    let mut bytes = vec![0; 12 + length as usize];
    bytes[..4].copy_from_slice(&port.to_le_bytes());
    bytes[4] = 0x80;
    bytes[5] = 6;
    bytes[7] = 2;
    bytes[10..12].copy_from_slice(&length.to_le_bytes());
    let returned = query(
        hub,
        IOCTL_USB_GET_DESCRIPTOR_FROM_NODE_CONNECTION,
        &mut bytes,
    )?;
    if returned < 12 + length as usize {
        return Err(wire::invalid("truncated USB configuration descriptor"));
    }
    Ok(bytes.split_off(12))
}
pub fn snapshot(request: &Path) -> io::Result<Device> {
    let request: Request = settings::read(request)?;
    if !request.identity.starts_with("USB\\") || request.node_instance.contains('\0') {
        return Err(wire::invalid("invalid USB snapshot identity"));
    }
    let (bus, port) = request
        .busid
        .split_once('-')
        .ok_or_else(|| wire::invalid("invalid physical USB port"))?;
    let bus: u32 = bus.parse().map_err(io::Error::other)?;
    let port: u32 = port.parse().map_err(io::Error::other)?;
    if bus == 0 || port == 0 {
        return Err(wire::invalid("invalid physical USB port"));
    }
    let hub = parent_hub(&request.node_instance)?;
    let mut bytes =
        vec![0; size_of::<USB_NODE_CONNECTION_INFORMATION_EX>() + 32 * size_of::<USB_PIPE_INFO>()];
    bytes[..4].copy_from_slice(&port.to_le_bytes());
    let returned = query(
        &hub,
        IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX,
        &mut bytes,
    )?;
    if returned < 35 {
        return Err(wire::invalid("truncated USB connection information"));
    }
    let info = unsafe {
        bytes
            .as_ptr()
            .cast::<USB_NODE_CONNECTION_INFORMATION_EX>()
            .read_unaligned()
    };
    if info.ConnectionStatus != DeviceConnected || info.DeviceIsHub {
        return Err(wire::invalid("selected port has no publishable USB device"));
    }
    let d = info.DeviceDescriptor;
    let vendor = d.idVendor;
    let product = d.idProduct;
    if d.bLength != 18
        || d.bDescriptorType != 1
        || !request
            .identity
            .to_ascii_uppercase()
            .contains(&format!("VID_{vendor:04X}&PID_{product:04X}"))
    {
        return Err(wire::invalid(
            "USB descriptor does not match the selected identity",
        ));
    }
    let mut speed = match info.Speed {
        0 => 2,
        1 => 1,
        2 => 3,
        3 => 5,
        _ => return Err(wire::invalid("unknown USB connection speed")),
    };
    let mut v2 = [0; 16];
    v2[..4].copy_from_slice(&port.to_le_bytes());
    v2[4..8].copy_from_slice(&16u32.to_le_bytes());
    v2[8..12].copy_from_slice(&7u32.to_le_bytes());
    if query(
        &hub,
        IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2,
        &mut v2,
    )
    .is_ok_and(|n| n >= 16)
        && u32::from_le_bytes(v2[12..16].try_into().unwrap()) & 5 != 0
    {
        speed = 5;
    }
    let header = configuration(&hub, port, 9)?;
    let length = u16::from_le_bytes([header[2], header[3]]);
    if length < 9 {
        return Err(wire::invalid("invalid USB configuration length"));
    }
    let (interfaces, _) = crate::usbdk::descriptors(&configuration(&hub, port, length)?)?;
    let device = Device {
        path: request.identity,
        busid: request.busid,
        bus,
        address: port,
        speed,
        vendor: d.idVendor,
        product: d.idProduct,
        revision: d.bcdDevice,
        class: d.bDeviceClass,
        subclass: d.bDeviceSubClass,
        protocol: d.bDeviceProtocol,
        configuration: info.CurrentConfigurationValue,
        configurations: d.bNumConfigurations,
        interfaces,
    };
    device.encode()?;
    Ok(device)
}
