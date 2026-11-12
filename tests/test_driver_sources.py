"""Validate the actual reviewed source pins before using synthetic archives."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("driver_source_packager", ROOT / "Package-DriverSources.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ConfiguredDriverSourceTests(unittest.TestCase):
    def test_real_configured_manifest_passes_strict_pin_validation(self):
        # Do not replace the configured hashes with synthetic fixture digests:
        # a mistyped real pin must fail CI even when no source cache is present.
        pins = module.load_pins(ROOT / "driver-sources.json")
        self.assertEqual(len(pins["sources"]), 3)

    def test_extra_leading_hex_digit_is_rejected(self):
        pins = module.load_pins(ROOT / "driver-sources.json")
        item = next(item for item in pins["sources"] if item["component"] == "usbip-win2")
        item["sha256"] = "2" + item["sha256"]
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "invalid-source-pins.json"
            path.write_text(json.dumps(pins), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "Invalid upstream source"):
                module.load_pins(path)


if __name__ == "__main__":
    unittest.main()
