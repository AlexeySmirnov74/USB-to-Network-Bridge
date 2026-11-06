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

BINARIES = ("usbbridge.exe", "usbbridge-client.exe", "usbbridge-server.exe", "usbbridge-agent.exe",
            "USB-to-Network-Bridge-Server.exe", "USB-to-Network-Bridge-Client.exe")

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

def verify_portable(portable, binaries):
    with zipfile.ZipFile(portable) as archive:
        entries = safe_names(archive)
        manifest_bytes = archive.read(entries["SHA256.json"])
        manifest = json.loads(manifest_bytes.decode("utf-8-sig"))
        expected = {}
        for item in manifest:
            name = item["File"].replace("\\", "/")
            if name in expected or name not in entries or name == "SHA256.json":
                raise ValueError("Invalid portable hash manifest")
            content = archive.read(entries[name])
            digest = hashlib.sha256(content).hexdigest()
            if digest != item["SHA256"].lower() or len(content) != item["Bytes"]:
                raise ValueError("Portable payload hash mismatch: " + name)
            if ".local." in name or Path(name).suffix.lower() in (".sys", ".cat", ".pfx", ".key", ".credential"):
                raise ValueError("Private data or driver binary in portable archive")
            expected[name] = (len(content), digest)
        if set(entries) != set(expected) | {"SHA256.json"}:
            raise ValueError("Unlisted portable archive content")
        for name in BINARIES:
            tested = hashlib.sha256((Path(binaries) / name).read_bytes()).hexdigest()
            if name not in expected or expected[name][1] != tested:
                raise ValueError("Portable executable differs from tested candidate: " + name)
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
    if set(files) != set(expected) | {"SHA256.json"}:
        raise ValueError("MSI files differ from the portable package")
    if files["SHA256.json"].read_bytes() != manifest_bytes:
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
    manifest, expected = verify_portable(args.portable, args.root / "target/release")
    verify_source(args.source, args.root, args.commit)
    if args.installed:
        verify_installed(args.installed, manifest, expected)
    print(json.dumps(dict(verified=True, source_commit=args.commit, payload_files=len(expected),
                          msi_payload_verified=bool(args.installed))))
if __name__ == "__main__":
    main()
