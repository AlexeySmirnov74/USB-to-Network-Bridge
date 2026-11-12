"""Verify release archives against reviewed Git blobs and tested application bytes."""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import zipfile

BINARIES = ("usbbridge.exe", "usbbridge-client.exe", "usbbridge-server.exe", "usbbridge-agent.exe", "usbbridge-camera.exe",
            "USB-to-Network-Bridge-Server.exe", "USB-to-Network-Bridge-Client.exe")
LAUNCHERS = frozenset({"USB-to-Network-Bridge-Server.exe", "USB-to-Network-Bridge-Client.exe"})
MANIFEST = "app/SHA256.json"
DRIVER_MANIFEST = "app/driver-installers.json"
DRIVER_PREFIX = "app/tools/driver-installers/"
TRUSTED_DRIVER_MANIFEST = Path(__file__).resolve().with_name("driver-installers.json")
HELPERS = ("Open-Installation.ps1", "Configure-Installation.ps1", "FirstRun-Identity.ps1", "Quit-Application.ps1",
           "Portable-Paths.ps1", "Portable-Installation.ps1", "Update-PortableSettings.ps1", "Relocate-Installation.ps1",
           "Migrate-PortableLayout.ps1", "Maintain-Installation.ps1", "Configure-UnityCapture.ps1", "Set-BackendProcessAccess.ps1", "Verify-DriverInstaller.ps1", "Ensure-ProductionDrivers.ps1")


def payload_binary(name):
    return name if name in LAUNCHERS else "app/" + name

def safe_names(archive):
    entries = {}
    folded = set()
    for item in archive.infolist():
        name = item.filename.replace("\\", "/")
        path = PurePosixPath(name)
        if (name.startswith("/") or ":" in name or ".." in path.parts
                or str(path) != name.rstrip("/") or name.casefold() in folded):
            raise ValueError("Unsafe or duplicate archive entry: " + name)
        folded.add(name.casefold())
        if not item.is_dir():
            entries[name] = item
    return entries

def driver_pins(manifest_path=None):
    """Read pins from the reviewed source tree, never from the candidate ZIP.

    An explicit trusted path supports isolated fixtures. Authenticode and signer
    checks belong to Verify-DriverInstaller.ps1 in the Windows packaging/release
    gates; this portable reader independently binds exact paths and bytes.
    """
    pins = json.loads(Path(manifest_path or TRUSTED_DRIVER_MANIFEST).read_text(encoding="utf-8-sig"))
    if not isinstance(pins, dict) or set(pins) != {"Server", "Client"}:
        raise ValueError("Expected exactly the two reviewed driver roles")
    paths = {}
    for role, suffix in (("Server", ".msi"), ("Client", ".exe")):
        entry = pins[role]
        if not isinstance(entry, dict):
            raise ValueError("Invalid reviewed driver pins")
        name = entry.get("file", "")
        if (not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9_.-]+", name)
                or PurePosixPath(name).suffix != suffix
                or not re.fullmatch(r"[a-fA-F0-9]{64}", entry.get("sha256", ""))
                or not isinstance(entry.get("signer"), str) or not entry["signer"]
                or ("signer_name" in entry and (not isinstance(entry["signer_name"], str) or not entry["signer_name"]))
                or not isinstance(entry.get("version"), str) or not entry["version"]):
            raise ValueError("Invalid reviewed driver filename, digest or signer")
        paths[DRIVER_PREFIX + name] = entry["sha256"].lower()
    return pins, paths


def verify_portable(portable, binaries, driver_manifest=None):
    pins, driver_paths = driver_pins(driver_manifest)
    application_paths = {payload_binary(name) for name in BINARIES}
    with zipfile.ZipFile(portable) as archive:
        entries = safe_names(archive)
        roots = {name for name in entries if "/" not in name}
        unexpected_directory = any(item.is_dir() and not item.filename.replace("\\", "/").startswith("app/")
                                   for item in archive.infolist())
        if (roots != LAUNCHERS or unexpected_directory
                or any(name not in LAUNCHERS and not name.startswith("app/") for name in entries)):
            raise ValueError("Portable root must contain only the two desktop EXEs and app")
        if MANIFEST not in entries:
            raise ValueError("Missing application hash manifest: " + MANIFEST)
        if DRIVER_MANIFEST not in entries:
            raise ValueError("Pinned driver manifest is missing")
        packaged_pins = json.loads(archive.read(entries[DRIVER_MANIFEST]).decode("utf-8-sig"))
        if packaged_pins != pins:
            raise ValueError("Pinned driver manifest differs from the reviewed source")
        for name in driver_paths:
            if name not in entries:
                raise ValueError("Pinned offline driver installer is missing: " + name)
        manifest_bytes = archive.read(entries[MANIFEST])
        manifest = json.loads(manifest_bytes.decode("utf-8-sig"))
        expected = {}
        for item in manifest:
            name = item["File"].replace("\\", "/")
            if name in expected or name not in entries or name == MANIFEST:
                raise ValueError("Invalid portable hash manifest")
            content = archive.read(entries[name])
            digest = hashlib.sha256(content).hexdigest()
            if digest != item["SHA256"].lower() or len(content) != item["Bytes"]:
                raise ValueError("Portable payload hash mismatch: " + name)
            if name in driver_paths and digest != driver_paths[name]:
                raise ValueError("Pinned driver installer hash mismatch: " + name)
            private_name = name.casefold()
            suffix = PurePosixPath(name).suffix.lower()
            if (".local." in private_name or private_name.endswith((".status.json", ".preferences.json", ".agent.lock", ".edit.lock", ".tray.lock"))
                    or suffix in (".sys", ".cat", ".pfx", ".key", ".pem", ".credential", ".jsonl", ".log")
                    or (suffix in (".exe", ".dll", ".msi") and name not in application_paths and name not in driver_paths)):
                raise ValueError("Private data or driver binary in portable archive: " + name)
            expected[name] = (len(content), digest)
        if set(entries) != set(expected) | {MANIFEST}:
            raise ValueError("Unlisted portable archive content")
        for name in BINARIES:
            tested = hashlib.sha256((Path(binaries) / name).read_bytes()).hexdigest()
            relative = payload_binary(name)
            if relative not in expected or expected[relative][1] != tested:
                raise ValueError("Portable executable differs from tested candidate: " + name)
        for helper in HELPERS:
            if "app/" + helper not in expected:
                raise ValueError("Portable runtime helper is missing: app/" + helper)
        return manifest_bytes, expected


def verify_source(source, root, commit):
    if not re.fullmatch(r"[0-9a-fA-F]{40}", commit):
        raise ValueError("Expected an exact Git commit")
    git = shutil.which("git") or str(Path(root) / "tools/mingit/cmd/git.exe")
    tree = subprocess.run([git, "-C", str(root), "ls-tree", "-r", "-z", commit],
                          check=True, capture_output=True).stdout
    expected = {}
    for row in tree.split(b"\0"):
        if not row:
            continue
        metadata, name = row.split(b"\t", 1)
        mode, kind, digest = metadata.split()
        if mode not in (b"100644", b"100755") or kind != b"blob":
            raise ValueError("Source tree contains a link or non-file object")
        expected["usbbridge/" + name.decode()] = digest.decode()
    with zipfile.ZipFile(source) as archive:
        entries = safe_names(archive)
        if set(entries) != set(expected):
            raise ValueError("Source ZIP paths differ from the reviewed commit")
        for name, digest in expected.items():
            content = archive.read(entries[name])
            actual = hashlib.sha1(b"blob " + str(len(content)).encode() + b"\0" + content).hexdigest()
            if actual != digest:
                raise ValueError("Source ZIP differs from reviewed commit: " + name)

def verify_installed(directory, manifest_bytes, expected):
    directory = Path(directory)
    files = {p.relative_to(directory).as_posix(): p for p in directory.rglob("*") if p.is_file()}
    if any(p.is_symlink() for p in directory.rglob("*")):
        raise ValueError("MSI extraction contains a link")
    if set(files) != set(expected) | {MANIFEST}:
        raise ValueError("MSI files differ from the portable package")
    if files[MANIFEST].read_bytes() != manifest_bytes:
        raise ValueError("MSI hash manifest differs from the portable package")
    for name, (size, digest) in expected.items():
        content = files[name].read_bytes()
        if len(content) != size or hashlib.sha256(content).hexdigest() != digest:
            raise ValueError("MSI payload differs from portable package: " + name)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--portable", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--installed", type=Path)
    args = parser.parse_args()
    manifest, expected = verify_portable(args.portable, args.root / "target/release", args.root / "driver-installers.json")
    verify_source(args.source, args.root, args.commit)
    if args.installed:
        verify_installed(args.installed, manifest, expected)
    print(json.dumps(dict(verified=True, source_commit=args.commit, payload_files=len(expected),
                          msi_payload_verified=bool(args.installed), driver_installer_hashes_verified=2)))
if __name__ == "__main__":
    main()
