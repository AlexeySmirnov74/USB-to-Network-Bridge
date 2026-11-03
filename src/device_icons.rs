//! System device-class icons. The image list owns copies; temporary HICONs are released.
use std::{io, ptr::null_mut};
use windows_sys::Win32::{
    Devices::DeviceAndDriverInstallation::*,
    UI::{Controls::*, Shell::*, WindowsAndMessaging::*},
};
pub struct Images(pub HIMAGELIST);
impl Images {
    pub fn new(size: i32) -> io::Result<Self> {
        unsafe {
            let list = Self(ImageList_Create(size, size, ILC_COLOR32 | ILC_MASK, 13, 0));
            if list.0 == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut computer = SHSTOCKICONINFO {
                cbSize: size_of::<SHSTOCKICONINFO>() as u32,
                ..std::mem::zeroed()
            };
            let owned =
                SHGetStockIconInfo(SIID_DESKTOPPC, SHGSI_ICON | SHGSI_SMALLICON, &mut computer)
                    >= 0;
            let icon = if owned {
                computer.hIcon
            } else {
                LoadIconW(null_mut(), IDI_APPLICATION)
            };
            let result = ImageList_ReplaceIcon(list.0, -1, icon);
            if owned {
                DestroyIcon(icon);
            }
            if result != 0 {
                return Err(io::Error::other("Cannot load computer icon"));
            }
            let classes = [
                GUID_DEVCLASS_USB,
                GUID_DEVCLASS_CAMERA,
                GUID_DEVCLASS_BLUETOOTH,
                GUID_DEVCLASS_DISKDRIVE,
                GUID_DEVCLASS_DISKDRIVE,
                GUID_DEVCLASS_MOUSE,
                GUID_DEVCLASS_KEYBOARD,
                GUID_DEVCLASS_HIDCLASS,
                GUID_DEVCLASS_MEDIA,
                GUID_DEVCLASS_PRINTER,
                GUID_DEVCLASS_NET,
                GUID_DEVCLASS_WPD,
            ];
            for (index, class) in classes.iter().enumerate() {
                let mut icon = null_mut();
                let mut stock: SHSTOCKICONINFO = std::mem::zeroed();
                stock.cbSize = size_of::<SHSTOCKICONINFO>() as u32;
                let loaded = if index == 4
                    && SHGetStockIconInfo(
                        SIID_DRIVEREMOVE,
                        SHGSI_ICON | SHGSI_SMALLICON,
                        &mut stock,
                    ) >= 0
                {
                    icon = stock.hIcon;
                    true
                } else {
                    SetupDiLoadClassIcon(class, &mut icon, null_mut()) != 0
                };
                let fallback = LoadIconW(null_mut(), IDI_APPLICATION);
                let result =
                    ImageList_ReplaceIcon(list.0, -1, if loaded { icon } else { fallback });
                if loaded {
                    DestroyIcon(icon);
                }
                if result != index as i32 + 1 {
                    return Err(io::Error::other("Cannot load device icon"));
                }
            }
            ImageList_SetBkColor(list.0, CLR_NONE as u32);
            Ok(list)
        }
    }
}
impl Drop for Images {
    fn drop(&mut self) {
        unsafe {
            ImageList_Destroy(self.0);
        }
    }
}
