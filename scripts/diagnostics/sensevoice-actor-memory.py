#!/usr/bin/env python3
"""Run one real desktop-actor test and sample its process tree. No GUI or mic.
No production manifest/key changes. All inputs are existing pinned public data.
The measured test host is NOT the shipping GUI app; do not claim its RAM gate.
"""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import queue
import subprocess
import tempfile
import threading
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
TEST = "session_runtime::actor::tests::tiled::real_tiled_import_restart_five_minutes_and_recovery_through_actor"
PREFIX = "SPEAKMORE_ACTOR_EVIDENCE "


def sha(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("candidate_report", type=pathlib.Path)
    args = parser.parse_args()
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise SystemExit("Requires Apple Silicon macOS")
    candidate = args.candidate_report.absolute()
    if not candidate.is_file() or any(p.is_symlink() for p in [candidate, *candidate.parents]):
        raise SystemExit("Candidate report must be a regular local file")
    if candidate.parent.parent != ROOT / ".cache/sensevoice/graph-study":
        raise SystemExit("Expected isolated graph-study candidate")
    parent = ROOT / ".cache/sensevoice/actor-study"
    if any(p.is_symlink() for p in [parent, *parent.parents]):
        raise SystemExit("Symlink output parent")
    parent.mkdir(parents=True, exist_ok=True)
    work = pathlib.Path(tempfile.mkdtemp(prefix="run-", dir=parent))
    print(json.dumps({"report_directory": str(work)}), flush=True)
    build = subprocess.run(["cargo", "test", "--offline", "-p", "speakmore-desktop", "--features", "session-fixtures,target-audit", "--lib", TEST, "--no-run", "--message-format=json"], cwd=ROOT, stdout=subprocess.PIPE, text=True, check=True)
    (work / "build.jsonl").write_text(build.stdout)
    artifacts = [json.loads(line) for line in build.stdout.splitlines()]
    executables = [r["executable"] for r in artifacts if r.get("reason") == "compiler-artifact" and r.get("target", {}).get("name") == "speakmore_desktop" and r.get("profile", {}).get("test") and r.get("executable")]
    if len(executables) != 1:
        raise RuntimeError("Expected one built desktop test executable")
    executable = pathlib.Path(executables[0])
    sampler = work / "process-tree-memory"
    c_source = ROOT / "scripts/diagnostics/macos-process-tree-memory.c"
    subprocess.run(["/usr/bin/clang", "-O2", "-Wall", "-Wextra", "-Werror", str(c_source), "-o", str(sampler)], check=True)
    # Preflight permissions on this known, live script process before inference.
    probe = subprocess.run([str(sampler), str(os.getpid())], stdout=subprocess.PIPE, text=True)
    if probe.returncode:
        raise RuntimeError("Process metrics unavailable; no inference was started")
    rows = [json.loads(line) for line in probe.stdout.splitlines()]
    if not rows or rows[0]["pid"] != os.getpid() or rows[0]["rss_bytes"] <= 0:
        raise RuntimeError("Invalid sampler preflight")
    sources = [c_source, ROOT / "scripts/diagnostics/sensevoice-actor-memory.py", ROOT / "src-tauri/src/session_runtime/tests/tiled.rs", candidate, executable]
    workers = ROOT / "target/release/bundle/macos/SpeakMore.app/Contents/Resources/workers"
    sources.extend(workers / n for n in ["speakmore-asr-worker", "speakmore-sensevoice-worker"])
    report = {"schema": 1, "completed": False, "scope": "desktop actor test host + descendant CPU workers; no WebView, GUI, microphone or real-time capture", "profile": "debug test host with release packaged workers", "platform": platform.platform(), "sample_interval_target_ms": 100, "sources": {str(p.relative_to(ROOT)): sha(p) for p in sources}, "samples": 0, "incomplete_samples": 0, "events": [], "stage_peaks": {}, "identities": {}, "errors": []}
    report_path = work / "report.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    env = dict(os.environ, SPEAKMORE_TEST_TILED_REPORT=str(candidate))
    child = subprocess.Popen([str(executable), TEST, "--exact", "--ignored", "--nocapture", "--test-threads=1"], cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
    messages = queue.Queue()
    def read_output():
        with (work / "stdout.log").open("w") as output:
            for line in child.stdout:
                output.write(line)
                output.flush()
                messages.put(line)
    reader = threading.Thread(target=read_output, daemon=True)
    reader.start()
    started = time.monotonic()
    root_identity = None
    stage = "starting"
    last_sample = None
    report["root_pid"] = child.pid
    try:
        with (work / "samples.jsonl").open("w") as output:
            while child.poll() is None:
                cycle = time.monotonic()
                while not messages.empty():
                    line = messages.get_nowait()
                    print(line, end="", flush=True)
                    if PREFIX in line:
                        event = json.loads(line.split(PREFIX, 1)[1])
                        report["events"].append({"elapsed_ms": round((cycle-started)*1000), **event})
                        stage = event.get("stage", stage)
                metric = subprocess.run([str(sampler), str(child.pid)], stdout=subprocess.PIPE, text=True, timeout=3)
                end = time.monotonic()
                processes = [json.loads(line) for line in metric.stdout.splitlines()]
                valid = metric.returncode == 0 and bool(processes)
                if processes:
                    current = (processes[0]["pid"], processes[0]["started"])
                    if root_identity is None:
                        root_identity = current
                    if current != root_identity:
                        raise RuntimeError("Root PID identity changed")
                row = {"elapsed_ms": round((cycle-started)*1000), "read_window_ms": round((end-cycle)*1000, 3), "stage": stage, "complete": valid, "processes": processes}
                output.write(json.dumps(row) + "\n")
                if valid:
                    report["samples"] += 1
                    peak = report["stage_peaks"].setdefault(stage, {"max_sum_rss_bytes": 0, "max_sum_footprint_bytes": 0, "max_host_rss_bytes": 0, "max_host_footprint_bytes": 0, "max_worker_count": 0, "samples": 0})
                    peak["samples"] += 1
                    peak["max_sum_rss_bytes"] = max(peak["max_sum_rss_bytes"], sum(p["rss_bytes"] for p in processes))
                    peak["max_sum_footprint_bytes"] = max(peak["max_sum_footprint_bytes"], sum(p["footprint_bytes"] for p in processes))
                    peak["max_host_rss_bytes"] = max(peak["max_host_rss_bytes"], processes[0]["rss_bytes"])
                    peak["max_host_footprint_bytes"] = max(peak["max_host_footprint_bytes"], processes[0]["footprint_bytes"])
                    peak["max_worker_count"] = max(peak["max_worker_count"], len(processes)-1)
                    for p in processes:
                        identity = f'{p["pid"]}:{p["started"]}'
                        record = report["identities"].setdefault(identity, {"pid": p["pid"], "parent": p["parent"], "started": p["started"], "max_rss_bytes": 0, "max_footprint_bytes": 0, "lifetime_peak_footprint_bytes": 0})
                        for target, source in [("max_rss_bytes", "rss_bytes"), ("max_footprint_bytes", "footprint_bytes"), ("lifetime_peak_footprint_bytes", "peak_footprint_bytes")]:
                            record[target] = max(record[target], p[source])
                else:
                    report["incomplete_samples"] += 1
                if last_sample is not None:
                    report["max_sample_gap_ms"] = max(report.get("max_sample_gap_ms", 0), round((cycle-last_sample)*1000, 3))
                last_sample = cycle
                if end-started > 900:
                    raise RuntimeError("Actor diagnostic exceeded 15-minute bound")
                time.sleep(max(0, 0.1-(end-cycle)))
        reader.join(timeout=5)
        while not messages.empty():
            line = messages.get_nowait()
            print(line, end="", flush=True)
            if PREFIX in line:
                report["events"].append({"elapsed_ms": round((time.monotonic()-started)*1000), **json.loads(line.split(PREFIX, 1)[1])})
        report["exit_code"] = child.returncode
        report["elapsed_seconds"] = round(time.monotonic()-started, 3)
        report["completed"] = child.returncode == 0 and any(e.get("completed") is True for e in report["events"]) and all(report["stage_peaks"].get(f"candidate_en300_{n}", {}).get("samples", 0) > 10 for n in (1, 2, 3))
    except BaseException as error:
        report["errors"].append(type(error).__name__ + ": " + str(error))
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        raise
    finally:
        report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"report": str(report_path), "completed": report["completed"]}), flush=True)
    if not report["completed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
