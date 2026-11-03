"""Archive the Git index; never walk local drivers, runtimes, logs or keys."""
from pathlib import Path
import json
import shutil
import subprocess
import zipfile
import tomllib

root = Path(__file__).resolve().parent
git = shutil.which("git") or str(root / "tools" / "mingit" / "cmd" / "git.exe")
result = subprocess.run([git, "ls-files", "--cached", "-z"], cwd=root, check=True, capture_output=True)
paths = [Path(name.decode("utf-8")) for name in result.stdout.split(b"\0") if name]
if not paths:
    raise RuntimeError("The Git index is empty. Review and stage public sources first.")
for relative in paths:
    path = root / relative
    if not path.resolve().is_relative_to(root):
        raise ValueError("Source path escapes project: " + str(relative))
    if relative.parts[0] in ("tools", "vendor", "drivers", "production", "target", "dist", "logs"):
        raise ValueError("Local artifact was staged: " + str(relative))
    if ".local." in path.name or path.name.endswith((".status.json", ".preferences.json", ".agent.lock", ".edit.lock", ".tray.lock")):
        raise ValueError("Private configuration or runtime state was staged: " + str(relative))
    if path.suffix.lower() in (".pfx", ".key", ".pem", ".cer", ".exe", ".dll", ".sys", ".cat", ".msi", ".zip", ".inventory", ".log", ".credential", ".jsonl"):
        raise ValueError("Binary, runtime data or signing key was staged: " + str(relative))
    if not path.is_file():
        raise FileNotFoundError(path)
version = tomllib.loads(subprocess.run([git,"show",":Cargo.toml"],cwd=root,check=True,capture_output=True).stdout.decode())["package"]["version"]
output = root / "dist" / f"USB-to-Network-Bridge-{version}-source.zip"
output.parent.mkdir(exist_ok=True)
with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
    for relative in sorted(paths):
        # Read the staged blob, so later private edits in the working tree
        # cannot enter an archive advertised as the reviewed Git index.
        blob = subprocess.run([git, "show", ":" + relative.as_posix()], cwd=root,
                              check=True, capture_output=True).stdout
        archive.writestr("usbbridge/" + relative.as_posix(), blob)
print(json.dumps({"archive": str(output), "files": len(paths), "bytes": output.stat().st_size}))
