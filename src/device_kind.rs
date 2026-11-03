//! Classification is shared with the remote catalog; never infer a type from a VID alone.
use crate::wire::Device;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    #[default]
    Usb,
    Camera,
    Bluetooth,
    Disk,
    Removable,
    Mouse,
    Keyboard,
    Input,
    Audio,
    Printer,
    Network,
    Phone,
}
impl DeviceKind {
    pub fn classify(name: &str, device: Option<&Device>) -> Self {
        let name = name.to_lowercase();
        let contains = |words: &[&str]| words.iter().any(|s| name.contains(s));
        let interfaces: Vec<_> = device
            .map(|d| {
                std::iter::once([d.class, d.subclass, d.protocol, 0])
                    .chain(d.interfaces.iter().copied())
                    .collect()
            })
            .unwrap_or_default();
        let class = |class| interfaces.iter().any(|i| i[0] == class);
        if class(14) || contains(&["camera", "webcam", "камера", "摄像"]) {
            Self::Camera
        } else if interfaces.iter().any(|i| i[..3] == [0xe0, 1, 1])
            || contains(&["bluetooth", "блютус", "蓝牙"])
        {
            Self::Bluetooth
        } else if contains(&["flash", "thumb drive", "флеш", "usb stick", "u盘"]) {
            Self::Removable
        } else if class(8)
            || contains(&[
                "hard disk",
                "hard drive",
                "mass storage",
                "ssd",
                "жестк",
                "жёстк",
            ])
        {
            Self::Disk
        } else if interfaces.iter().any(|i| i[..3] == [3, 1, 2])
            || contains(&["mouse", "мышь", "мышка", "maus", "souris", "鼠标"])
        {
            Self::Mouse
        } else if interfaces.iter().any(|i| i[..3] == [3, 1, 1])
            || contains(&["keyboard", "клавиат", "tastatur", "clavier", "键盘"])
        {
            Self::Keyboard
        } else if class(3) || contains(&["input", "hid", "ввода"]) {
            Self::Input
        } else if class(1) || contains(&["audio", "microphone", "speaker", "микрофон"]) {
            Self::Audio
        } else if class(7) || contains(&["printer", "принтер", "drucker", "imprimante"]) {
            Self::Printer
        } else if class(2) || class(10) || contains(&["ethernet", "wi-fi", "network adapter"]) {
            Self::Network
        } else if contains(&["phone", "android", "iphone", "mtp", "телефон"]) {
            Self::Phone
        } else {
            Self::Usb
        }
    }
    pub fn image(self) -> i32 {
        self as i32 + 1 // slot zero is the computer/server icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composite_interfaces_and_fallback_names() {
        let mut device = crate::backend::mock_device();
        device.class = 0xef;
        device.interfaces = vec![[1, 1, 0, 0], [14, 1, 0, 0]];
        assert_eq!(
            DeviceKind::classify("USB Composite", Some(&device)),
            DeviceKind::Camera
        );
        device.interfaces = vec![[3, 1, 2, 0]];
        assert_eq!(
            DeviceKind::classify("USB Input Device", Some(&device)),
            DeviceKind::Mouse
        );
        assert_eq!(
            DeviceKind::classify("USB Flash Drive", None),
            DeviceKind::Removable
        );
        assert_eq!(
            DeviceKind::classify("Wireless Bluetooth", None),
            DeviceKind::Bluetooth
        );
        assert_eq!(DeviceKind::classify("Unknown USB", None), DeviceKind::Usb);
        device.interfaces = vec![[8, 6, 80, 0]];
        assert_eq!(
            DeviceKind::classify("USB storage", Some(&device)),
            DeviceKind::Disk
        );
    }
}
