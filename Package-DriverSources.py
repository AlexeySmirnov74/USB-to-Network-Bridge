"""Build the corresponding-source ZIP from pinned, previously downloaded files.

Uses only the Python standard library, no network, installer, compiler or driver.
Precompressed upstream archives are stored unchanged with bounded streaming IO.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile
import tomllib
from urllib.parse import urlsplit
import zipfile


CHUNK = 1024 * 1024
COMPONENTS = {"VirtualBox", "usbipd-win", "usbip-win2"}


def digest(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as source:
        while block := source.read(CHUNK):
            result.update(block)
    return result.hexdigest()


def load_pins(path):
    manifest = json.loads(Path(path).read_text(encoding="utf-8-sig"))
    sources = manifest.get("sources")
    if (manifest.get("schema") != 1 or not isinstance(sources, list)
            or len(sources) != 3 or {item.get("component") for item in sources} != COMPONENTS):
        raise ValueError("Expected exactly three reviewed upstream source archives")
    names = set()
    for item in sources:
        name, checksum = item.get("file"), item.get("sha256")
        url = urlsplit(item.get("url", ""))
        if (not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9_.-]+", name)
                or not name.endswith((".zip", ".tar.bz2")) or name.casefold() in names
                or not isinstance(checksum, str) or not re.fullmatch(r"[a-fA-F0-9]{64}", checksum)
                or not isinstance(item.get("version"), str) or not item["version"]
                or not isinstance(item.get("license"), str) or not item["license"]
                or url.scheme != "https" or url.username or url.password
                or url.hostname not in {"download.virtualbox.org", "codeload.github.com"}):
            raise ValueError("Invalid upstream source name, digest or provenance")
        names.add(name.casefold())
    return manifest


def member(name):
    value = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
    value.compress_type = zipfile.ZIP_STORED
    value.create_system = 3
    value.external_attr = 0o100644 << 16
    return value


def package_sources(root, source_directory):
    root, source_directory = Path(root).resolve(), Path(source_directory).resolve()
    manifest_path = root / "driver-sources.json"
    pins = load_pins(manifest_path)
    version = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8-sig"))["package"]["version"]
    if not re.fullmatch(r"[A-Za-z0-9.+-]+", version):
        raise ValueError("Invalid application version")
    sources = []
    for item in pins["sources"]:
        path = source_directory / item["file"]
        if path.is_symlink() or not path.is_file():
            raise ValueError("Pinned upstream source archive is missing or linked: " + item["file"])
        if digest(path) != item["sha256"].lower():
            raise ValueError("Pinned upstream source hash mismatch: " + item["file"])
        sources.append((item, path, path.stat().st_size))

    # Read only public, explicitly named provenance and the public text notices.
    text_paths = [Path("driver-sources.json"), Path("driver-installers.json"),
                  Path("THIRD-PARTY.txt"), Path("docs/driver-sources.md")]
    licenses = root / "licenses/drivers"
    if not licenses.is_dir() or licenses.is_symlink():
        raise ValueError("Public upstream driver license notices are missing")
    notices = []
    for path in sorted(licenses.rglob("*")):
        if path.is_symlink():
            raise ValueError("Driver license notices must not contain links")
        if path.is_file():
            if ".local." in path.name.lower() or path.suffix.lower() not in {".md", ".txt", ".license"}:
                raise ValueError("Only public text notices belong in licenses/drivers")
            notices.append(path.relative_to(root))
    if not notices:
        raise ValueError("Public upstream driver license notices are empty")
    text_paths += notices
    texts = {}
    for relative in text_paths:
        path = root / relative
        if path.is_symlink() or not path.is_file() or path.stat().st_size > CHUNK:
            raise ValueError("Missing, linked or excessive public provenance file: " + relative.as_posix())
        texts[relative.as_posix()] = path.read_bytes()
    texts["README.txt"] = (
        "USB to Network Bridge " + version + " - corresponding upstream driver source\n\n"
        "The upstream/ directory contains three original, unmodified source archives.\n"
        "driver-sources.json records their exact versions, official URLs and SHA-256 hashes.\n"
        "provenance.json records the source bytes included in this package.\n"
        "driver-installers.json identifies the separately distributed signed installers.\n\n"
        "Extract the upstream ZIP/tar archives and follow their original build instructions.\n"
        "Their original licenses and copyright notices remain in force; additional notices\n"
        "are under licenses/drivers/. The MIT bridge application source is distributed\n"
        "separately in the matching application source ZIP. This archive is not needed\n"
        "to install or run the application offline.\n\n"
        "Rebuild with: python Package-DriverSources.py --source-directory <local source cache>\n"
        "The packaging command never downloads files or runs upstream build/install steps.\n"
    ).encode("utf-8")
    provenance = dict(schema=1, application_version=version,
        source_manifest_sha256=hashlib.sha256(texts["driver-sources.json"]).hexdigest(),
        sources=[dict(component=item["component"], version=item["version"],
            file="upstream/" + item["file"], bytes=size, sha256=item["sha256"].lower(), url=item["url"])
            for item, _, size in sources])
    texts["provenance.json"] = (json.dumps(provenance, indent=2, sort_keys=True) + "\n").encode()

    output = root / "dist" / f"USB-to-Network-Bridge-{version}-driver-sources.zip"
    output.parent.mkdir(parents=True, exist_ok=True)
    descriptor, partial = tempfile.mkstemp(prefix="driver-sources-", suffix=".partial", dir=output.parent)
    os.close(descriptor)
    try:
        with zipfile.ZipFile(partial, "w", allowZip64=True) as archive:
            for name, data in sorted(texts.items()):
                archive.writestr(member(name), data)
            for item, path, size in sources:
                entry = member("upstream/" + item["file"])
                entry.file_size = size
                copied, checksum = 0, hashlib.sha256()
                with path.open("rb") as source, archive.open(entry, "w", force_zip64=size >= zipfile.ZIP64_LIMIT) as target:
                    while block := source.read(CHUNK):
                        target.write(block)
                        checksum.update(block)
                        copied += len(block)
                if copied != size or checksum.hexdigest() != item["sha256"].lower():
                    raise ValueError("Upstream source changed while packaging: " + item["file"])
        os.replace(partial, output)
    finally:
        Path(partial).unlink(missing_ok=True)
    return dict(archive=str(output), files=len(texts) + len(sources),
                bytes=output.stat().st_size, sha256=digest(output), source_archives=3, network_used=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    root = Path(__file__).resolve().parent
    parser.add_argument("--source-directory", type=Path, default=root / "tools/driver-sources")
    args = parser.parse_args()
    print(json.dumps(package_sources(root, args.source_directory)))


if __name__ == "__main__":
    main()
