"""Reject payload substitution and ambiguous ZIP names without installing anything."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("release_artifacts", Path(__file__).parents[1] / "Verify-ReleaseArtifacts.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class ArtifactTests(unittest.TestCase):
    def package(self, root, *, replacements=None, additions=None, omissions=()):
        """Build an isolated candidate without invoking compilers or installers."""
        payload = {}
        for name in module.BINARIES:
            content = ("reviewed " + name).encode()
            (root / name).write_bytes(content)
            payload[module.payload_binary(name)] = content
        payload.update({"app/" + name: b"# runtime helper" for name in module.HELPERS})
        payload["app/docs/installation.md"] = b"Public documentation"
        # These fixtures test the independent archive/hash gate, not signature
        # validation. Windows packaging authenticates the real upstream bytes.
        pins = json.loads(module.TRUSTED_DRIVER_MANIFEST.read_text(encoding="utf-8-sig"))
        for role in ("Server", "Client"):
            content = ("reviewed fixture installer " + role).encode()
            pins[role]["sha256"] = hashlib.sha256(content).hexdigest()
            payload[module.DRIVER_PREFIX + pins[role]["file"]] = content
        (root / "trusted-driver-installers.json").write_text(json.dumps(pins), encoding="utf-8")
        payload[module.DRIVER_MANIFEST] = json.dumps(pins).encode()
        payload.update(replacements or {})
        payload.update(additions or {})
        for name in omissions:
            payload.pop(name, None)
        manifest = [dict(File=name, Bytes=len(content), SHA256=hashlib.sha256(content).hexdigest())
                    for name, content in payload.items()]
        manifest_bytes = json.dumps(manifest).encode()
        path = root / "package.zip"
        with zipfile.ZipFile(path, "w") as archive:
            for name, content in payload.items():
                archive.writestr(name, content)
            archive.writestr(module.MANIFEST, manifest_bytes)
        return path, payload, manifest_bytes

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
            path, _, _ = self.package(root, replacements={"app/usbbridge.exe": b"different"})
            with self.assertRaisesRegex(ValueError, "differs from tested"):
                module.verify_portable(path, root, root / "trusted-driver-installers.json")

    def test_msi_payload_substitution_is_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / "app").mkdir()
            (root / module.MANIFEST).write_bytes(b"manifest")
            (root / "app/usbbridge.exe").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "MSI payload differs"):
                module.verify_installed(root, b"manifest", {"app/usbbridge.exe": (8, hashlib.sha256(b"original").hexdigest())})

    def test_portable_and_msi_use_root_relative_app_manifest(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path, payload, manifest_bytes = self.package(root)
            result, expected = module.verify_portable(path, root, root / "trusted-driver-installers.json")
            self.assertEqual(result, manifest_bytes)
            self.assertIn("app/usbbridge.exe", expected)
            self.assertIn("USB-to-Network-Bridge-Server.exe", expected)
            installed = root / "installed"
            for name, content in {**payload, module.MANIFEST: manifest_bytes}.items():
                target = installed / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(content)
            module.verify_installed(installed, result, expected)

    def test_root_cannot_expose_internal_executable(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path, _, _ = self.package(root, additions={"usbbridge.exe": b"internal"})
            with self.assertRaisesRegex(ValueError, "root must contain only"):
                module.verify_portable(path, root, root / "trusted-driver-installers.json")

    def test_root_cannot_contain_extra_file_or_directory(self):
        for extra in ("README.md", "extra/"):
            with self.subTest(extra=extra), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                path, _, _ = self.package(root)
                with zipfile.ZipFile(path, "a") as archive:
                    archive.writestr(extra, b"")
                with self.assertRaisesRegex(ValueError, "root must contain only"):
                    module.verify_portable(path, root, root / "trusted-driver-installers.json")

    def test_runtime_helpers_are_required(self):
        for helper in ("FirstRun-Identity.ps1", "Migrate-PortableLayout.ps1", "Configure-UnityCapture.ps1", "Set-BackendProcessAccess.ps1",
                       "Portable-Paths.ps1", "Portable-Installation.ps1", "Update-PortableSettings.ps1", "Relocate-Installation.ps1", "Verify-DriverInstaller.ps1", "Ensure-ProductionDrivers.ps1"):
            with self.subTest(helper=helper), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                path, _, _ = self.package(root, omissions=("app/" + helper,))
                with self.assertRaisesRegex(ValueError, "helper is missing"):
                    module.verify_portable(path, root, root / "trusted-driver-installers.json")

    def test_private_app_files_are_rejected(self):
        for name in ("app/client.LOCAL.json", "app/keys/private.pem", "app/logs/client.debug.jsonl",
                     "app/client.STATUS.json", "app/upstream.sys"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                path, _, _ = self.package(root, additions={name: b"private"})
                with self.assertRaisesRegex(ValueError, "Private data or driver binary"):
                    module.verify_portable(path, root, root / "trusted-driver-installers.json")

    def test_both_pinned_offline_installers_are_required(self):
        pins, paths = module.driver_pins()
        for name in paths:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                path, _, _ = self.package(root, omissions=(name,))
                with self.assertRaisesRegex(ValueError, "installer is missing"):
                    module.verify_portable(path, root, root / "trusted-driver-installers.json")

    def test_coherent_archive_manifest_cannot_hide_corrupt_installer(self):
        _, paths = module.driver_pins()
        for name in paths:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                path, _, _ = self.package(root, replacements={name: b"corrupt installer"})
                with self.assertRaisesRegex(ValueError, "Pinned driver installer hash mismatch"):
                    module.verify_portable(path, root, root / "trusted-driver-installers.json")

    def test_candidate_cannot_supply_its_own_driver_pins(self):
        for change in ("sha256", "signer", "signer_name", "file"):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                bad, _ = module.driver_pins()
                bad["Server"][change] = {"sha256": "0" * 64, "signer": "Unreviewed signer", "signer_name": "Unreviewed CN", "file": "another.msi"}[change]
                path, _, _ = self.package(root, replacements={module.DRIVER_MANIFEST: json.dumps(bad).encode()})
                with self.assertRaisesRegex(ValueError, "manifest differs"):
                    module.verify_portable(path, root, root / "trusted-driver-installers.json")

    def test_installer_directory_does_not_allow_other_binaries(self):
        for name in (module.DRIVER_PREFIX + "another.exe", module.DRIVER_PREFIX + "another.msi",
                     module.DRIVER_PREFIX + "helper.dll", "app/usbipd-win_5.3.0_x64.msi",
                     "app/docs/USBip-0.9.8.1-x64.exe", "app/extra.exe"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                path, _, _ = self.package(root, additions={name: b"unreviewed binary"})
                with self.assertRaisesRegex(ValueError, "Private data or driver binary"):
                    module.verify_portable(path, root, root / "trusted-driver-installers.json")

    def test_valid_offline_archive_verification_never_fetches_or_launches(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path, _, _ = self.package(root)
            with patch("socket.create_connection", side_effect=AssertionError("network used")), \
                    patch("subprocess.run", side_effect=AssertionError("process launched")):
                _, expected = module.verify_portable(path, root, root / "trusted-driver-installers.json")
            _, pins = module.driver_pins(root / "trusted-driver-installers.json")
            self.assertTrue(set(pins).issubset(expected))

    def test_reviewed_manifest_must_not_allow_paths_or_extra_roles(self):
        for mutation in ("path", "extra_role", "hash", "empty_signer", "empty_signer_name"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                pins, _ = module.driver_pins()
                if mutation == "path":
                    pins["Client"]["file"] = "../another.exe"
                elif mutation == "extra_role":
                    pins["Extra"] = pins["Client"]
                elif mutation == "hash":
                    pins["Client"]["sha256"] = "not-a-digest"
                elif mutation == "empty_signer":
                    pins["Client"]["signer"] = ""
                else:
                    pins["Client"]["signer_name"] = ""
                manifest = root / "pins.json"
                manifest.write_text(json.dumps(pins), encoding="utf-8")
                with self.assertRaises(ValueError):
                    module.driver_pins(manifest)


if __name__ == "__main__":
    unittest.main()
