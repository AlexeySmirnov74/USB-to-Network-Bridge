//! Machine DPAPI permits the SYSTEM recovery agent to use credentials saved by the UI.
//! An explicit protected file DACL grants access only to its owner, SYSTEM and administrators.
use crate::proxy::Credentials;
use std::{
    io,
    path::{Path, PathBuf},
};
#[cfg(windows)]
fn crypt(data: &[u8], protect: bool) -> io::Result<Vec<u8>> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(bytes)
}
#[cfg(windows)]
pub fn save(auth: &Credentials) -> io::Result<PathBuf> {
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("LOCALAPPDATA is unavailable"))?
        .join("USBBridge")
        .join("credentials");
    save_at(&root, auth)
}
#[cfg(windows)]
fn create_private_file(path: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
    use windows_sys::Win32::{
        Foundation::{INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL},
    };
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let sddl: Vec<u16> = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;OW)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            0x40000000,
            1,
            &security,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    let error = if handle == INVALID_HANDLE_VALUE {
        Some(io::Error::last_os_error())
    } else {
        None
    };
    unsafe {
        LocalFree(descriptor);
    }
    if let Some(error) = error {
        return Err(error);
    }
    Ok(unsafe { std::fs::File::from_raw_handle(handle) })
}
#[cfg(windows)]
pub fn save_at(root: &Path, auth: &Credentials) -> io::Result<PathBuf> {
    use std::io::Write;
    if auth.username.len() > 4096 || auth.password.len() > 4096 {
        return Err(io::Error::other("Proxy credentials exceed 4096 bytes"));
    }
    std::fs::create_dir_all(root)?;
    let path = root.join(format!("{}.credential", uuid::Uuid::new_v4()));
    let encrypted = crypt(&serde_json::to_vec(auth).map_err(io::Error::other)?, true)?;
    let mut file = create_private_file(&path)?;
    file.write_all(&encrypted)?;
    file.sync_all()?;
    Ok(path)
}
/// An explicit export file for a shared video key, private from its creation.
pub fn save_key(path: &Path, secret: &str) -> io::Result<()> {
    use std::io::Write;
    #[cfg(windows)]
    let mut file = create_private_file(path)?;
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?
    };
    #[cfg(not(any(windows, unix)))]
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(secret.as_bytes())?;
    file.sync_all()
}
#[cfg(windows)]
pub fn load(path: &Path) -> io::Result<Credentials> {
    let data = std::fs::read(path)?;
    if data.len() > 65536 {
        return Err(io::Error::other("Invalid proxy credential file"));
    }
    serde_json::from_slice(&crypt(&data, false)?)
        .map_err(|_| io::Error::other("Invalid encrypted proxy credentials"))
}
#[cfg(not(windows))]
pub fn load(_path: &Path) -> io::Result<Credentials> {
    Err(io::Error::other("Proxy credentials require Windows DPAPI"))
}
#[cfg(not(windows))]
pub fn save(_auth: &Credentials) -> io::Result<PathBuf> {
    Err(io::Error::other("Proxy credentials require Windows DPAPI"))
}
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn protected_credentials_roundtrip_without_plaintext_on_disk() {
        let dir = std::env::temp_dir().join(format!("usbbridge-secrets-{}", uuid::Uuid::new_v4()));
        let auth = Credentials {
            username: "fixture-user".into(),
            password: "fixture-password-only".into(),
        };
        let path = save_at(&dir, &auth).unwrap();
        let data = std::fs::read(&path).unwrap();
        assert!(
            !data
                .windows(auth.password.len())
                .any(|v| v == auth.password.as_bytes())
        );
        let got = load(&path).unwrap();
        assert_eq!(got.username, auth.username);
        assert_eq!(got.password, auth.password);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
