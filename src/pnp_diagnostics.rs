//! Windows USB/HID PnP metadata. The OS callback never performs I/O or waits.
use std::{
    ffi::c_void,
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
#[derive(Debug)]
struct Notice {
    action: i32,
    time: u64,
    identity: String,
    interface: Option<String>,
    class: Option<String>,
}
enum Message {
    Notice(Notice),
    Stop,
}
struct Context {
    sender: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
}
pub struct Watch {
    handles: Vec<usize>,
    context: Box<Context>,
    worker: Option<JoinHandle<()>>,
}

fn instance_from_interface(path: &str) -> Option<String> {
    let mut parts = path.strip_prefix("\\\\?\\")?.split('#');
    let bus = parts.next()?;
    let hardware = parts.next()?;
    let serial = parts.next()?;
    if !bus.eq_ignore_ascii_case("USB") && !bus.eq_ignore_ascii_case("HID") {
        return None;
    }
    Some(format!(
        "{}\\{hardware}\\{serial}",
        bus.to_ascii_uppercase()
    ))
}
fn decode(action: i32, bytes: &[u8]) -> Option<Notice> {
    if bytes.len() < 8 || bytes.len() > 65536 {
        return None;
    }
    let kind = i32::from_le_bytes(bytes[..4].try_into().ok()?);
    let offset = match kind {
        CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE => 24,
        CM_NOTIFY_FILTER_TYPE_DEVICEINSTANCE => 8,
        _ => return None,
    };
    let tail = bytes.get(offset..)?;
    let words: Vec<u16> = tail
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    let end = words.iter().position(|c| *c == 0)?;
    let name = String::from_utf16(&words[..end]).ok()?;
    let (identity, interface, class) = if kind == CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE {
        (
            instance_from_interface(&name)?,
            Some(name),
            Some(uuid::Uuid::from_bytes_le(bytes[8..24].try_into().ok()?).to_string()),
        )
    } else {
        let bus = name.split('\\').next()?;
        if !bus.eq_ignore_ascii_case("USB") && !bus.eq_ignore_ascii_case("HID") {
            return None;
        }
        (name, None, None)
    };
    Some(Notice {
        action,
        time: crate::recovery::timestamp(),
        identity,
        interface,
        class,
    })
}
unsafe extern "system" fn callback(
    _handle: HCMNOTIFICATION,
    context: *const c_void,
    action: CM_NOTIFY_ACTION,
    data: *const CM_NOTIFY_EVENT_DATA,
    length: u32,
) -> u32 {
    if context.is_null() || data.is_null() || !(8..=65536).contains(&length) {
        return 0;
    }
    let context = unsafe { &*context.cast::<Context>() };
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length as usize) };
    if let Some(notice) = decode(action, bytes)
        && context.sender.try_send(Message::Notice(notice)).is_err()
    {
        context.dropped.fetch_add(1, Ordering::Relaxed);
    }
    0
}
fn action_name(action: i32) -> &'static str {
    match action {
        0 => "interface_arrival",
        1 => "interface_removal",
        2 => "query_remove",
        3 => "query_remove_failed",
        4 => "remove_pending",
        5 => "remove_complete",
        7 => "instance_enumerated",
        8 => "instance_started",
        9 => "instance_removed",
        _ => "other",
    }
}
fn node_state(identity: &str) -> serde_json::Value {
    let id: Vec<u16> = identity.encode_utf16().chain(Some(0)).collect();
    let mut node = 0;
    let result = unsafe { CM_Locate_DevNodeW(&mut node, id.as_ptr(), CM_LOCATE_DEVNODE_PHANTOM) };
    if result != 0 {
        return serde_json::json!({"lookup_configret":result});
    }
    let (mut status, mut problem) = (0, 0);
    let result = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, node, 0) };
    let mut service = [0u16; 256];
    let mut length = (service.len() * 2) as u32;
    let service_result = unsafe {
        CM_Get_DevNode_Registry_PropertyW(
            node,
            CM_DRP_SERVICE,
            std::ptr::null_mut(),
            service.as_mut_ptr().cast(),
            &mut length,
            0,
        )
    };
    let service = (service_result == 0).then(|| {
        String::from_utf16_lossy(
            &service[..service
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(service.len())],
        )
    });
    let mut parent = 0;
    let mut parent_name = [0u16; 512];
    let parent_id = if unsafe { CM_Get_Parent(&mut parent, node, 0) } == 0
        && unsafe {
            CM_Get_Device_IDW(
                parent,
                parent_name.as_mut_ptr(),
                parent_name.len() as u32,
                0,
            )
        } == 0
    {
        Some(String::from_utf16_lossy(
            &parent_name[..parent_name
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(parent_name.len())],
        ))
    } else {
        None
    };
    serde_json::json!({"state_sample_unix_ms":crate::recovery::timestamp(),"status_configret":result,"status_flags":format!("0x{status:08x}"),"started":result==0&&status&DN_STARTED!=0,"has_problem":result==0&&status&DN_HAS_PROBLEM!=0,"will_be_removed":result==0&&status&DN_WILL_BE_REMOVED!=0,"problem_code":problem,"driver_service":service,"parent_identity":parent_id})
}
pub fn snapshot(identity: &str) {
    crate::diagnostics::event(
        "windows.usb.snapshot",
        serde_json::json!({"identity":identity,"node":node_state(identity)}),
    );
}
impl Watch {
    pub fn start() -> Self {
        let (sender, receiver) = mpsc::sync_channel(512);
        let dropped = Arc::new(AtomicU64::new(0));
        let context = Box::new(Context {
            sender,
            dropped: dropped.clone(),
        });
        let worker=thread::Builder::new().name("windows-usb-pnp".into()).spawn(move||{
            while let Ok(Message::Notice(notice))=receiver.recv() {
                crate::diagnostics::event("windows.usb.pnp",serde_json::json!({"action":action_name(notice.action),"action_code":notice.action,"event_unix_ms":notice.time,"identity":notice.identity,"interface_path":notice.interface,"interface_class":notice.class,"node":node_state(&notice.identity),"dropped_since_previous":dropped.swap(0,Ordering::Relaxed)}));
            }
        });
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(error) => {
                crate::diagnostics::event(
                    "windows.usb.watch.failed",
                    serde_json::json!({"error":error.to_string()}),
                );
                None
            }
        };
        let mut handles = Vec::new();
        if worker.is_some() {
            for (kind, flags) in [
                (
                    CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
                    CM_NOTIFY_FILTER_FLAG_ALL_INTERFACE_CLASSES,
                ),
                (
                    CM_NOTIFY_FILTER_TYPE_DEVICEINSTANCE,
                    CM_NOTIFY_FILTER_FLAG_ALL_DEVICE_INSTANCES,
                ),
            ] {
                let filter = CM_NOTIFY_FILTER {
                    cbSize: size_of::<CM_NOTIFY_FILTER>() as u32,
                    Flags: flags,
                    FilterType: kind,
                    ..Default::default()
                };
                let mut handle = std::ptr::null_mut();
                let result = unsafe {
                    CM_Register_Notification(
                        &filter,
                        (&*context as *const Context).cast(),
                        Some(callback),
                        &mut handle,
                    )
                };
                if result == 0 {
                    handles.push(handle as usize);
                } else {
                    crate::diagnostics::event(
                        "windows.usb.watch.registration.failed",
                        serde_json::json!({"filter":kind,"configret":result}),
                    );
                }
            }
        }
        crate::diagnostics::event(
            "windows.usb.watch.started",
            serde_json::json!({"registrations":handles.len(),"queue_capacity":512,"payload_logging":false}),
        );
        Self {
            handles,
            context,
            worker,
        }
    }
}
impl Default for Watch {
    fn default() -> Self {
        Self::start()
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            unsafe {
                CM_Unregister_Notification(handle as HCMNOTIFICATION);
            }
        }
        let _ = self.context.sender.send(Message::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn data(kind: i32, name: &str) -> Vec<u8> {
        let mut bytes = vec![0; if kind == 0 { 24 } else { 8 }];
        bytes[..4].copy_from_slice(&kind.to_le_bytes());
        for c in name.encode_utf16().chain(Some(0)) {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
        bytes
    }
    #[test]
    fn callback_parser_bounds_and_filters_to_usb_hid_metadata() {
        let bytes = data(
            0,
            "\\\\?\\USB#VID_1209&PID_F001#fixture#{00000000-0000-0000-0000-000000000000}",
        );
        let event = decode(0, &bytes).unwrap();
        assert_eq!(event.identity, "USB\\VID_1209&PID_F001\\fixture");
        for length in 0..bytes.len() - 1 {
            assert!(decode(0, &bytes[..length]).is_none());
        }
        assert!(decode(0, &data(0, "\\\\?\\DISPLAY#example#monitor#{guid}")).is_none());
        assert!(decode(8, &data(2, "USB\\VID_1209&PID_F001\\fixture")).is_some());
        assert!(decode(8, &data(2, "PCI\\unrelated")).is_none());
    }
    #[test]
    fn notifications_register_and_unregister_without_usb_mutation() {
        assert_eq!(std::mem::offset_of!(CM_NOTIFY_EVENT_DATA, u), 8);
        let watcher = Watch::start();
        assert_eq!(watcher.handles.len(), 2);
        drop(watcher);
    }
}
