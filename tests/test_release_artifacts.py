"""Reject payload substitution and ambiguous ZIP names without installing anything."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location("release_artifacts", Path(__file__).parents[1] / "Verify-ReleaseArtifacts.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class ArtifactTests(unittest.TestCase):
    def test_duplicate_case_is_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "bad.zip"
            with zipfile.ZipFile(path, "w") as archive:
                archive.writestr("App.exe", b"one")
                archive.writestr("app.exe", b"two")
            with zipfile.ZipFile(path) as archive, self.assertRaises(ValueError):
                module.safe_names(archive)

    def test_parent_path_is_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "bad.zip"
            with zipfile.ZipFile(path, "w") as archive:
                archive.writestr("../bad.exe", b"bad")
            with zipfile.ZipFile(path) as archive, self.assertRaises(ValueError):
                module.safe_names(archive)

    def test_payload_cannot_replace_tested_binary(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            manifest = []
            path = root / "package.zip"
            with zipfile.ZipFile(path, "w") as archive:
                for name in module.BINARIES:
                    (root / name).write_bytes(b"tested")
                    archive.writestr(name, b"different")
                    manifest.append(dict(File=name, Bytes=9, SHA256=hashlib.sha256(b"different").hexdigest()))
                archive.writestr("SHA256.json", json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "differs from tested"):
                module.verify_portable(path, root)

    def test_msi_payload_substitution_is_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / "SHA256.json").write_bytes(b"manifest")
            (root / "app.exe").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "MSI payload differs"):
                module.verify_installed(root, b"manifest", {"app.exe": (8, hashlib.sha256(b"original").hexdigest())})

if __name__ == "__main__":
    unittest.main()
