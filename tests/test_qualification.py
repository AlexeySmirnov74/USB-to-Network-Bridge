import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec=importlib.util.spec_from_file_location("qualification",Path(__file__).resolve().parents[1]/"Analyze-Qualification.py")
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)

class QualificationTests(unittest.TestCase):
    def fixture(self, root, *, completed=True, growth=0, elapsed=48*3600):
        binary=[dict(file="usbbridge.exe",sha256="A"*64)]
        manifest=dict(role="Client",requested_seconds=48*3600,binaries=binary)
        result=dict(role="Client",requested_seconds=48*3600,elapsed_seconds=elapsed,
            phase="completed" if completed else "interrupted",passed=completed,
            config_unchanged=True,camera_test=True,stalls=0,low_fps_windows=0,no_images_saved=True,
            last_camera=dict(stats=dict(frames=100000,decoded=100000,error=0)),binaries=binary)
        (root/"manifest.local.json").write_text(json.dumps(manifest))
        (root/"result.local.json").write_text(json.dumps(result))
        with (root/"samples.local.jsonl").open("w") as stream:
            for i in range(240):
                t=i*elapsed/239
                point=dict(name="usbbridge",pid=1,started_utc="test",
                    private_bytes=int(30*1048576+growth*1048576*i/239),handles=100,threads=6)
                stream.write(json.dumps(dict(elapsed_seconds=t,processes=[point]))+"\n")

    def test_short_or_interrupted_run_is_never_a_production_pass(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);self.fixture(root,elapsed=120)
            self.assertFalse(module.analyze(root)["passed"])
            self.fixture(root,completed=False)
            self.assertFalse(module.analyze(root)["passed"])

    def test_sustained_memory_growth_fails(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);self.fixture(root,growth=512)
            result=module.analyze(root)
            self.assertFalse(result["passed"])
            self.assertTrue(any("resource growth" in p for p in result["problems"]))

    def test_stable_complete_series_passes_without_exporting_raw_logs(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);self.fixture(root)
            result=module.analyze(root)
            self.assertTrue(result["passed"],result["problems"])
            self.assertFalse(result["raw_logs_included"])

    def test_nonfinite_duration_cannot_pass(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);self.fixture(root)
            file=root/"result.local.json";result=json.loads(file.read_text());result["elapsed_seconds"]=float("nan");file.write_text(json.dumps(result))
            with self.assertRaises(ValueError):
                module.analyze(root)

    def test_explicit_short_observation_does_not_claim_completed_soak(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);self.fixture(root,completed=False,elapsed=1240)
            report=module.analyze(root,20/60,allow_operator_stop=True)
            self.assertTrue(report["passed"],report["problems"])
            self.assertTrue(report["operator_stop_accepted"])
            self.assertFalse(report["requested_run_completed"])
            self.assertFalse(report["full_48h_soak_passed"])
            self.assertFalse(module.analyze(root)["passed"])

    def test_operator_stop_never_hides_a_real_failure(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);self.fixture(root,completed=False,elapsed=1240)
            p=root/"result.local.json";result=json.loads(p.read_text())
            result["error"]="capture failed";p.write_text(json.dumps(result))
            self.assertFalse(module.analyze(root,20/60,allow_operator_stop=True)["passed"])

    def test_short_memory_windows_do_not_cancel_each_other(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);self.fixture(root,completed=False,elapsed=1240,growth=20)
            p=root/"samples.local.jsonl"
            samples=p.read_text().splitlines()[::6]
            p.write_text("\n".join(samples)+"\n")
            report=module.analyze(root,20/60,allow_operator_stop=True)
            self.assertGreater(report["resources"][0]["private_growth_mib"],1)

    def test_camera_errors_fail_even_when_frames_exist(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);self.fixture(root)
            file=root/"result.local.json";result=json.loads(file.read_text())
            result["last_camera"]["stats"]["error"]=1;file.write_text(json.dumps(result))
            self.assertFalse(module.analyze(root)["passed"])

if __name__=="__main__":
    unittest.main()
