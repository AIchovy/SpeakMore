#!/usr/bin/env python3
"""Observe an already-running test app's resource coalition. Never launches,
controls or terminates the app. No microphone, UI automation or process content.
Private read-only coalition ABI is verified by two censuses and kernel counts;
incomplete samples remain evidence, never contribute to the reported peaks.
"""
import argparse
import hashlib
import json
import pathlib
import platform
import plistlib
import subprocess
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=pathlib.Path)
    parser.add_argument("--seconds", type=int, default=300)
    args = parser.parse_args()
    if platform.system() != "Darwin" or not 1 <= args.seconds <= 1800:
        raise SystemExit("Requires macOS; duration must be 1..1800 seconds")
    app = args.app.absolute()
    if app.parent != ROOT / "target/release/bundle/macos" or app.suffix != ".app":
        raise SystemExit("Expected a locally built SpeakMore test app")
    if any(p.is_symlink() for p in [app, *app.parents]):
        raise SystemExit("Symlink app path")
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    if info["CFBundleIdentifier"] != "com.speakmore.modelcheck.20260916":
        raise SystemExit("Only the isolated model check identity is allowed")
    executable = app / "Contents/MacOS" / info["CFBundleExecutable"]
    parent = ROOT / ".cache/app-memory-study"
    if any(p.is_symlink() for p in [parent, *parent.parents]):
        raise SystemExit("Symlink output path")
    parent.mkdir(parents=True, exist_ok=True)
    work = pathlib.Path(tempfile.mkdtemp(prefix="run-", dir=parent))
    sampler = work / "app-memory"
    source = ROOT / "scripts/diagnostics/macos-app-memory.c"
    subprocess.run(["/usr/bin/clang", "-O2", "-Wall", "-Wextra", "-Werror", str(source), "-o", str(sampler)], check=True)
    probe = subprocess.run([str(sampler), str(executable)], capture_output=True, text=True, timeout=3)
    (work / "preflight.json").write_text(probe.stdout)
    if probe.returncode:
        raise SystemExit("App/coalition unavailable; inspect " + str(work / "preflight.json"))
    first = json.loads(probe.stdout)
    expected = str(first["root_started"])
    files = [source, pathlib.Path(__file__), executable, sampler]
    files.extend((app / "Contents/Resources/workers").glob("speakmore-*-worker"))
    report = {
        "schema": 1, "sampling_finished": False,
        "scope": "isolated GUI app resource coalition; synthetic public audio only if that explicit build is running",
        "claim": "Sampled process counters only; not physical recording, natural speech quality, minimum hardware or a release gate",
        "platform": platform.platform(), "app_id": info["CFBundleIdentifier"],
        "started_epoch_ms": round(time.time() * 1000),
        "app": str(app.relative_to(ROOT)), "root_pid": first["root_pid"],
        "root_started": first["root_started"], "coalition": first["resource_coalition"],
        "sources": {str(p.relative_to(ROOT)): sha(p) for p in files},
        "target_interval_ms": 100, "requested_seconds": args.seconds,
        "complete_samples": 0, "incomplete_samples": 0, "identities": {},
        "max_sum_rss_bytes": 0, "max_sum_footprint_bytes": 0,
        "max_main_rss_bytes": 0, "max_members": 0,
        "max_read_window_ms": 0, "max_sample_gap_ms": 0, "errors": [],
    }
    started = time.monotonic()
    last = None
    print(json.dumps({"report_directory": str(work), "root_pid": first["root_pid"], "members": len(first["processes"])}), flush=True)
    try:
        with (work / "samples.jsonl").open("w") as output:
            while time.monotonic() - started < args.seconds:
                cycle = time.monotonic()
                result = subprocess.run([str(sampler), str(executable), expected], capture_output=True, text=True, timeout=3)
                ended = time.monotonic()
                row = json.loads(result.stdout)
                if result.returncode == 0 and not row.get("complete"):
                    raise RuntimeError("Sampler status mismatch")
                row["elapsed_ms"] = round((cycle - started) * 1000, 3)
                row["read_window_ms"] = round((ended - cycle) * 1000, 3)
                output.write(json.dumps(row) + "\n")
                output.flush()
                report["max_read_window_ms"] = max(report["max_read_window_ms"], row["read_window_ms"])
                if last is not None:
                    report["max_sample_gap_ms"] = max(report["max_sample_gap_ms"], round((cycle - last) * 1000, 3))
                last = cycle
                if row["complete"] and result.returncode == 0:
                    if row["root_pid"] != first["root_pid"] or row["resource_coalition"] != first["resource_coalition"]:
                        raise RuntimeError("Root identity or coalition changed")
                    report["complete_samples"] += 1
                    processes = row["processes"]
                    report["max_members"] = max(report["max_members"], len(processes))
                    for field in ("rss_bytes", "footprint_bytes"):
                        key = "max_sum_" + field
                        value = sum(p[field] for p in processes)
                        if value > report[key]:
                            report[key] = value
                            report[key + "_at_ms"] = row["elapsed_ms"]
                    for p in processes:
                        if p["pid"] == row["root_pid"]:
                            report["max_main_rss_bytes"] = max(report["max_main_rss_bytes"], p["rss_bytes"])
                        key = str(p["pid"]) + ":" + str(p["started"])
                        identity = report["identities"].setdefault(key, {**p, "first_seen_ms": row["elapsed_ms"]})
                        identity["last_seen_ms"] = row["elapsed_ms"]
                        for metric in ("rss_bytes", "footprint_bytes", "peak_footprint_bytes"):
                            identity[metric] = max(identity[metric], p[metric])
                else:
                    report["incomplete_samples"] += 1
                    if row.get("reason") in ("app_not_running", "root_identity_changed", "app_coalition_not_isolated"):
                        report["ended_by"] = row["reason"]
                        break
                time.sleep(max(0, 0.1 - (ended - cycle)))
        report["sampling_finished"] = True
    except BaseException as error:
        report["errors"].append(type(error).__name__ + ": " + str(error))
        raise
    finally:
        report["elapsed_seconds"] = round(time.monotonic() - started, 3)
        report["raw_sha256"] = sha(work / "samples.jsonl")
        (work / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"report": str(work / "report.json"), "samples": report["complete_samples"], "incomplete": report["incomplete_samples"]}), flush=True)


if __name__ == "__main__":
    main()
