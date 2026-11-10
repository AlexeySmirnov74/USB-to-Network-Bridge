// Write the documented Win32 .res format directly; no extra resource compiler.
use std::{env, fs, path::PathBuf};
fn u16le(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes[..2].try_into().unwrap())
}
fn u32le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[..4].try_into().unwrap())
}
fn resource(output: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    for value in [data.len() as u32, 32] {
        output.extend(value.to_le_bytes());
    }
    for value in [0xffff, kind, 0xffff, id] {
        output.extend(value.to_le_bytes());
    }
    output.extend(0u32.to_le_bytes());
    for value in [0x1030u16, 0x409] {
        output.extend(value.to_le_bytes());
    }
    output.extend([0u8; 8]);
    output.extend(data);
    while !output.len().is_multiple_of(4) {
        output.push(0);
    }
}
fn version_block(
    key: &str,
    kind: u16,
    value: &[u8],
    value_length: u16,
    children: &[Vec<u8>],
) -> Vec<u8> {
    let mut output = vec![0; 2];
    output.extend(value_length.to_le_bytes());
    output.extend(kind.to_le_bytes());
    for ch in key.encode_utf16().chain(Some(0)) {
        output.extend(ch.to_le_bytes());
    }
    while !output.len().is_multiple_of(4) {
        output.push(0);
    }
    output.extend(value);
    for child in children {
        while !output.len().is_multiple_of(4) {
            output.push(0);
        }
        output.extend(child);
    }
    let length = u16::try_from(output.len()).unwrap();
    output[..2].copy_from_slice(&length.to_le_bytes());
    output
}
fn version_info() -> Vec<u8> {
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numbers: Vec<u32> = version.split('.').map(|s| s.parse().unwrap()).collect();
    let mut fixed = Vec::new();
    for value in [
        0xfeef04bd,
        0x10000,
        numbers[0] << 16 | numbers[1],
        numbers[2] << 16,
        numbers[0] << 16 | numbers[1],
        numbers[2] << 16,
        0x3f,
        0,
        0x40004,
        1,
        0,
        0,
        0,
    ] {
        fixed.extend(value.to_le_bytes());
    }
    let strings = [
        ("FileDescription", "USB to Network Bridge"),
        ("ProductName", "USB to Network Bridge"),
        ("FileVersion", version.as_str()),
        ("ProductVersion", version.as_str()),
        (
            "Comments",
            "https://github.com/AlexeySmirnov74/USB-to-Network-Bridge",
        ),
    ]
    .iter()
    .map(|(key, value)| {
        let words: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
        let data: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        version_block(key, 1, &data, words.len() as u16, &[])
    })
    .collect::<Vec<_>>();
    let table = version_block("040904B0", 1, &[], 0, &strings);
    let strings = version_block("StringFileInfo", 1, &[], 0, &[table]);
    let translation = version_block("Translation", 0, &[9, 4, 0xb0, 4], 4, &[]);
    let variables = version_block("VarFileInfo", 1, &[], 0, &[translation]);
    version_block(
        "VS_VERSION_INFO",
        0,
        &fixed,
        fixed.len() as u16,
        &[strings, variables],
    )
}
fn main() {
    println!("cargo:rerun-if-changed=assets/usbbridge.ico");
    println!("cargo:rerun-if-changed=src/camera_native.cpp");
    println!("cargo:rerun-if-changed=Build-CameraPreview.ps1");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let native_directory = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let status = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            "Build-CameraPreview.ps1",
            "-OutputDirectory",
        ])
        .arg(&native_directory)
        .status()
        .expect("start camera bridge compiler");
    assert!(status.success(), "camera bridge compilation failed");
    println!(
        "cargo:rustc-link-search=native={}",
        native_directory.display()
    );
    for name in [
        "static=usbbridge_camera",
        "strmiids",
        "ole32",
        "oleaut32",
        "setupapi",
        "cfgmgr32",
    ] {
        println!("cargo:rustc-link-lib={name}");
    }
    let icon = fs::read("assets/usbbridge.ico")
        .expect("assets/usbbridge.ico is required; run Generate-Icon.ps1");
    assert_eq!(&icon[..4], &[0, 0, 1, 0]);
    let count = u16le(&icon[4..]);
    let mut group = icon[..6].to_vec();
    let mut output = Vec::new();
    resource(&mut output, 0, 0, &[]);
    for index in 0..count as usize {
        let entry = &icon[6 + index * 16..6 + (index + 1) * 16];
        let length = u32le(&entry[8..]) as usize;
        let start = u32le(&entry[12..]) as usize;
        let id = index as u16 + 1;
        resource(&mut output, 3, id, &icon[start..start + length]);
        group.extend(&entry[..12]);
        group.extend(id.to_le_bytes());
    }
    resource(&mut output, 14, 101, &group);
    let manifest = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<assemblyIdentity version="APP_VERSION.0" processorArchitecture="amd64" name="USBtoNetworkBridge" type="win32"/>
<description>USB to Network Bridge</description>
<dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency>
<trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security></trustInfo>
<application xmlns="urn:schemas-microsoft-com:asm.v3"><windowsSettings><dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware></windowsSettings></application>
</assembly>"#;
    let manifest = manifest.replace("APP_VERSION", &env::var("CARGO_PKG_VERSION").unwrap());
    resource(&mut output, 24, 1, manifest.as_bytes());
    resource(&mut output, 16, 1, &version_info());
    let destination = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("usbbridge.res");
    fs::write(&destination, output).unwrap();
    println!("cargo:rustc-link-arg-bins={}", destination.display());
}
