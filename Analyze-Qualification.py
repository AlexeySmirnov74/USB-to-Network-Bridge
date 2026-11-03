"""Analyze bounded qualification metadata. Never reads camera images or credentials."""
from __future__ import annotations
import argparse
from collections import deque
import json
import math
from pathlib import Path

def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))

def analyze(directory: Path, minimum_hours: float = 48, allow_operator_stop: bool = False):
    if not math.isfinite(minimum_hours) or minimum_hours <= 0:
        raise ValueError('minimum_hours must be finite and positive')
    directory = Path(directory)
    manifest = read_json(directory / "manifest.local.json")
    result = read_json(directory / "result.local.json")
    problems = []
    required = minimum_hours * 3600
    if not all(isinstance(result.get(key), (int, float)) and math.isfinite(result[key]) for key in ('elapsed_seconds', 'requested_seconds')):
        raise ValueError('Evidence duration must be finite')
    if manifest.get('role') != result.get('role'):
        problems.append('manifest role mismatch')
    if not result.get('config_unchanged'):
        problems.append('configuration changed during qualification')
    complete = result.get("phase") == "completed" and result.get("passed") is True
    operator_stop = (allow_operator_stop and result.get("phase") == "interrupted"
                     and result.get("error") is None)
    if not complete and not operator_stop:
        problems.append("run did not complete successfully")
    if result.get("requested_seconds", 0) < required or result.get("elapsed_seconds", 0) < required - 2:
        problems.append("required observation duration has not elapsed")
    if result.get("role") == "Client" and not result.get("camera_test"):
        problems.append("client evidence has no continuous camera capture")
    if result.get("stalls", 0) or result.get("low_fps_windows", 0):
        problems.append("camera stalls or low-FPS windows were recorded")
    if result.get("camera_test"):
        stats = (result.get("last_camera") or {}).get("stats", {})
        if stats.get("frames", 0) <= 0 or stats.get("decoded", 0) <= 0 or stats.get("error", 0):
            problems.append("valid decoded camera frames were not confirmed")
    if not result.get("no_images_saved"):
        problems.append("image-free capture policy is not confirmed")
    if result.get("binaries") != manifest.get("binaries"):
        problems.append("binary manifest mismatch")
    warmup = 600 if result.get("requested_seconds", 0) >= 3600 else 0
    groups = {}
    sample_count = 0
    for name in ("samples.local.jsonl.4", "samples.local.jsonl.3", "samples.local.jsonl.2",
                 "samples.local.jsonl.1", "samples.local.jsonl"):
        path = directory / name
        if not path.exists():
            continue
        with path.open(encoding="utf-8-sig") as stream:
            for line in stream:
                sample = json.loads(line)
                t = float(sample["elapsed_seconds"])
                sample_count += 1
                if t < warmup:
                    continue
                for process in sample["processes"]:
                    key = (process["name"], process["pid"], process["started_utc"])
                    g = groups.setdefault(key, dict(first=[], last=deque(maxlen=60), n=0,
                        sx=0., sy=0., sxx=0., sxy=0., first_t=t, last_t=t))
                    point = (t, process["private_bytes"], process["handles"], process["threads"])
                    if len(g["first"]) < 60:
                        g["first"].append(point)
                    g["last"].append(point)
                    x = (t - g["first_t"]) / 3600
                    y = process["private_bytes"] / 1048576
                    g["n"] += 1
                    g["sx"] += x; g["sy"] += y; g["sxx"] += x*x; g["sxy"] += x*y
                    g["last_t"] = t
    if sample_count < 2:
        problems.append("insufficient resource samples")
    summaries = []
    long_groups = {}
    for (name, pid, started), g in groups.items():
        duration = g["last_t"] - g["first_t"]
        if duration < min(60, result.get("elapsed_seconds", 0) / 2):
            continue
        def mean(rows, column):
            return sum(row[column] for row in rows) / len(rows)
        # Keep the comparison windows separate even for a short observation.
        edge = min(60, max(1, g["n"] // 4))
        first = g["first"][:edge]
        last = list(g["last"])[-edge:]
        growth = (mean(last, 1) - mean(first, 1)) / 1048576
        handles = mean(last, 2) - mean(first, 2)
        threads = mean(last, 3) - mean(first, 3)
        denominator = g["n"] * g["sxx"] - g["sx"]**2
        slope = ((g["n"] * g["sxy"] - g["sx"] * g["sy"]) / denominator
                 if denominator > 0 else 0)
        bad = growth > 128 or handles > 256 or threads > 8
        if duration >= 6*3600 and growth > 32 and slope > 1:
            bad = True
        if bad:
            problems.append(f"resource growth exceeds limits: {name}")
        summaries.append(dict(component=name, pid=pid, observed_seconds=duration,
            private_growth_mib=round(growth,3), private_slope_mib_per_hour=round(slope,3),
            handles_growth=round(handles,2), threads_growth=round(threads,2), passed=not bad))
        if duration >= max(60, result.get("elapsed_seconds", 0) * .1):
            long_groups.setdefault(name, []).append((g["first_t"], g["last_t"]))
    # Disjoint long-lived segments expose restarts without counting short console operations.
    for name, intervals in long_groups.items():
        ordered = sorted(intervals)
        if len(ordered) > 1 and any(b[0] >= a[1] for a,b in zip(ordered, ordered[1:])):
            problems.append(f"long-lived component restarted: {name}")
    if not summaries:
        problems.append("no long-lived process resource series")
    return dict(schema=1, passed=not problems, role=result.get("role"),
        minimum_hours=minimum_hours, elapsed_seconds=result.get("elapsed_seconds",0),
        original_phase=result.get("phase"), requested_run_completed=complete,
        operator_stop_accepted=bool(operator_stop),
        full_48h_soak_passed=bool(complete and result.get("elapsed_seconds",0) >= 48*3600-2 and not problems),
        camera_test=bool(result.get("camera_test")), samples=sample_count,
        camera_summary=(dict(average_fps=result["last_camera"].get("average_fps"),
            frames=result["last_camera"]["stats"].get("frames"),
            decoded=result["last_camera"]["stats"].get("decoded"),
            errors=result["last_camera"]["stats"].get("error"),
            width=result["last_camera"]["stats"].get("width"),
            height=result["last_camera"]["stats"].get("height")) if result.get("last_camera") else None),
        resources=summaries, problems=problems, binaries=result.get("binaries",[]),
        raw_logs_included=False)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("directory",type=Path)
    parser.add_argument("--minimum-hours",type=float,default=48)
    parser.add_argument("--output",type=Path)
    parser.add_argument("--allow-operator-stop", action="store_true", help="Analyze a deliberately stopped observation; never changes the original evidence")
    args=parser.parse_args()
    if not math.isfinite(args.minimum_hours) or args.minimum_hours <= 0:
        parser.error("minimum-hours must be positive")
    report=analyze(args.directory,args.minimum_hours,args.allow_operator_stop)
    text=json.dumps(report,indent=2)+"\n"
    if args.output:
        args.output.write_text(text,encoding="utf-8")
    print(text,end="")
    return 0 if report["passed"] else 1

if __name__=="__main__":
    raise SystemExit(main())
