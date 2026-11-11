//! Machine DPAPI permits the SYSTEM recovery agent to use credentials saved by the UI.
//! A protected file DACL grants access to the creating token's user, SYSTEM and administrators.
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
struct LocalAllocation(*mut std::ffi::c_void);
#[cfg(windows)]
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.0);
        }
    }
}
#[cfg(windows)]
fn token_user_sid() -> io::Result<String> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_QUERY, TOKEN_USER,
            TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut raw_token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(raw_token) };
    let mut bytes = 0u32;
    let result = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut bytes,
        )
    };
    if result == 0 && io::Error::last_os_error().raw_os_error() != Some(122) {
        return Err(io::Error::last_os_error());
    }
    if bytes < std::mem::size_of::<TOKEN_USER>() as u32 || bytes > 65536 {
        return Err(io::Error::other("Invalid Windows token user buffer length"));
    }
    // Word storage meets TOKEN_USER's pointer alignment, unlike a byte vector.
    let mut storage = vec![0usize; (bytes as usize).div_ceil(std::mem::size_of::<usize>())];
    let capacity = (storage.len() * std::mem::size_of::<usize>()) as u32;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            storage.as_mut_ptr().cast(),
            capacity,
            &mut bytes,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if bytes < std::mem::size_of::<TOKEN_USER>() as u32 || bytes > capacity {
        return Err(io::Error::other("Invalid Windows token user result length"));
    }
    let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    let mut text = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _text = LocalAllocation(text.cast());
    // Windows string SIDs are short; reject an unexpectedly unbounded result.
    let mut wide = Vec::new();
    for index in 0..256 {
        let ch = unsafe { *text.add(index) };
        if ch == 0 {
            return String::from_utf16(&wide).map_err(io::Error::other);
        }
        wide.push(ch);
    }
    Err(io::Error::other("Windows token user SID is too long"))
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
    // An elevated process may create files owned by Administrators. OWNER
    // RIGHTS alone then excludes that same account's limited scheduled token.
    let sddl: Vec<u16> = format!("D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;{})", token_user_sid()?)
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
    fn new_key_dacl_explicitly_grants_token_user_without_owner_or_world_access() {
        use std::os::windows::process::CommandExt;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let path =
            std::env::temp_dir().join(format!("usbbridge-key-acl-{}.key", uuid::Uuid::new_v4()));
        let _cleanup = Cleanup(path.clone());
        // This synthetic value is never printed or read by the ACL inspector.
        save_key(&path, "unused-fixture-key-for-acl-test").unwrap();
        let output=std::process::Command::new("powershell.exe")
            .args(["-NoProfile","-NonInteractive","-Command"])
            .arg(r"$ErrorActionPreference='Stop';$acl=Get-Acl -LiteralPath $env:USB_BRIDGE_TEST_ACL_FILE;[ordered]@{protected=$acl.AreAccessRulesProtected;grants=@($acl.GetAccessRules($true,$false,[Security.Principal.SecurityIdentifier])|ForEach-Object{[ordered]@{sid=$_.IdentityReference.Value;allow=($_.AccessControlType-eq 'Allow');full=($_.FileSystemRights-band [Security.AccessControl.FileSystemRights]::FullControl)-eq [Security.AccessControl.FileSystemRights]::FullControl}})}|ConvertTo-Json -Depth 4 -Compress")
            .env("USB_BRIDGE_TEST_ACL_FILE",&path)
            .creation_flags(0x08000000)
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let acl: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(acl["protected"], true);
        let creator = token_user_sid().unwrap();
        let grants = acl["grants"].as_array().unwrap();
        assert!(
            grants
                .iter()
                .any(|ace| ace["sid"].as_str() == Some(creator.as_str()))
        );
        for ace in grants {
            assert_eq!(ace["allow"], true);
            assert_eq!(ace["full"], true);
            let sid = ace["sid"].as_str().unwrap();
            assert!(
                sid == creator || sid == "S-1-5-18" || sid == "S-1-5-32-544",
                "unexpected private-file ACL trustee"
            );
            assert_ne!(sid, "S-1-3-4"); // OWNER RIGHTS is not the creating user.
            assert_ne!(sid, "S-1-1-0"); // Everyone.
        }
    }
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
