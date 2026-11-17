#!/usr/bin/env python3
"""Offline, developer-only native allocation experiment on pinned public audio.
Never starts SpeakMore, reads user history, opens a microphone or uses a key.
Each configuration runs in a fresh subprocess; no product setting is changed.
"""
import argparse
import csv
import hashlib
import json
import os
import pathlib
import platform
import subprocess
import tempfile
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
MODES = {
    "default": "",
    "no_arena": "EnableCpuMemArena=0\n",
    "no_arena_or_pattern": "EnableCpuMemArena=0\nEnableMemPattern=0\n",
    "no_pattern": "EnableMemPattern=0\n",
    "node_stats": "EnableMemPattern=0\n",
    "exact_arena": "EnableMemPattern=0\nSessionConfig.session.use_env_allocators=1\n",
    "compact_arena": "EnableMemPattern=0\nSessionConfig.session.use_env_allocators=1\n",
    "growth_arena": "EnableMemPattern=0\nSessionConfig.session.use_env_allocators=1\n",
}
ORT_INPUTS = {
    "onnxruntime_c_api.h": (394991, "b69a133143c0da61782b2b02cc8a620d21ac139231fd211719776d10385135c1"),
    "onnxruntime_error_code.h": (2775, "5ce3b054e798eced8d14f5b86e98692fd33470463f96194ce0700a2d53dd8721"),
    "onnxruntime_ep_c_api.h": (152985, "db86df0f846b8d3bdbf429a5c76c5805293196575ca3c475878612591cd18e51"),
    "LICENSE": (1073, "2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c"),
}


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def verify(path, size, sha):
    require(path.is_file() and not path.is_symlink(), "missing or non-regular input: " + path.name)
    require(path.stat().st_size == size and digest(path) == sha, "input differs from pinned hash: " + path.name)


def prepare_headers():
    """Explicit opt-in download; normal experiment runs remain offline."""
    destination = ROOT / ".cache/sensevoice/memory-study/ort-api"
    for path in [*reversed(destination.relative_to(ROOT).parents), destination.relative_to(ROOT)]:
        require(not (ROOT / path).is_symlink(), "refusing symlink output directory")
    destination.mkdir(parents=True, exist_ok=True)
    for name, (size, sha) in ORT_INPUTS.items():
        path = destination / name
        if path.is_file() and not path.is_symlink() and path.stat().st_size == size and digest(path) == sha:
            continue
        require(not path.is_symlink(), "refusing symlink header")
        suffix = name if name == "LICENSE" else "include/onnxruntime/core/session/" + name
        url = "https://raw.githubusercontent.com/microsoft/onnxruntime/v1.28.2/" + suffix
        with urllib.request.urlopen(url, timeout=30) as response:
            require(response.status == 200 and response.url.startswith("https://"), "expected HTTPS 200")
            raw = response.read(size + 1)
        require(len(raw) == size and hashlib.sha256(raw).hexdigest() == sha, "download differs from pinned header")
        with tempfile.NamedTemporaryFile(dir=destination, delete=False) as pending:
            temporary = pathlib.Path(pending.name)
            try:
                pending.write(raw)
                pending.flush()
                os.replace(temporary, path)
            finally:
                temporary.unlink(missing_ok=True)
    print("Verified optional ONNX Runtime 1.28.2 diagnostic headers and license.")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repeats", type=int, choices=(1, 2, 3), default=1)
    parser.add_argument("--modes", nargs="+", choices=tuple(MODES), default=["default", "no_arena", "no_arena_or_pattern"])
    parser.add_argument("--prepare-ort-headers", action="store_true", help="download pinned optional diagnostic headers only; no experiment")
    parser.add_argument("--candidate-report", type=pathlib.Path, help="developer-only tiled graph report; never changes the signed product catalog")
    args = parser.parse_args()
    require(len(set(args.modes)) == len(args.modes), "duplicate experiment mode")
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise SystemExit("This experiment requires Apple Silicon macOS.")
    if args.prepare_ort_headers:
        prepare_headers()
        return
    lock = json.loads((ROOT / "resources/sensevoice-native.lock.json").read_text())
    libs = ROOT / ".cache/sensevoice/native" / lock["directory"] / "lib"
    header = ROOT / "vendor/sherpa-onnx/c-api.h"
    for path, record in [(header, lock["header"])] + [(libs / r["filename"], r) for r in lock["libraries"]]:
        verify(path, record["size_bytes"], record["sha256"])
    model_root = ROOT / ".cache/sensevoice/model"
    catalog = json.loads((ROOT / "resources/model-catalog.payload.json").read_text())
    model = next(m for m in catalog["models"] if m["model_id"] == "sensevoice-small-int8")
    for artifact in model["artifacts"]:
        path = model_root / artifact["filename"]
        verify(path, artifact["size_bytes"], artifact["sha256"])
    model_path = model_root / "model.int8.onnx"
    candidate_evidence = None
    if args.candidate_report:
        report_path = args.candidate_report.absolute()
        require(not report_path.is_symlink() and report_path.is_file(), "invalid candidate report")
        require(report_path.parent.parent == ROOT / ".cache/sensevoice/graph-study", "candidate must be in isolated graph-study cache")
        require(not any(p.is_symlink() for p in report_path.parents), "symlink candidate directory")
        evidence = json.loads(report_path.read_text())
        require(evidence.get("completed") is True and evidence.get("source_sha256") == digest(model_path) and evidence.get("layers") == 70, "candidate generation is incomplete or uses another model")
        model_path = report_path.parent / "model.tiled.onnx"
        verify(model_path, evidence["candidate_bytes"], evidence["candidate_sha256"])
        candidate_evidence = {"report_sha256": digest(report_path), "block": evidence["block"], "candidate_sha256": evidence["candidate_sha256"]}
    audio = model_root / "test_wavs/en.wav"
    # This fixture is checked against the repository's pinned public sources.
    fixture_lock = json.loads((ROOT / "resources/sensevoice-fixtures.lock.json").read_text())
    audio_record = next(r for r in fixture_lock["files"] if r["filename"] == "test_wavs/en.wav")
    verify(audio, audio_record["size_bytes"], audio_record["sha256"])
    parent = ROOT / ".cache/sensevoice/memory-study"
    parent.mkdir(parents=True, exist_ok=True)
    work = pathlib.Path(tempfile.mkdtemp(prefix="run-", dir=parent))
    executable = work / "sensevoice-memory"
    extra_flags = []
    if any(m in args.modes for m in ("exact_arena", "compact_arena", "growth_arena")):
        ort_headers = parent / "ort-api"
        for name, (size, sha) in ORT_INPUTS.items():
            verify(ort_headers / name, size, sha)
        extra_flags = ["-DSPEAKMORE_MEMORY_ARENA_EXPERIMENT", "-I" + str(ort_headers)]
    names = ["sherpa-onnx-c-api", "sherpa-onnx-core", "sherpa-onnx-kaldifst-core", "sherpa-onnx-fstfar", "sherpa-onnx-fst", "kaldi-decoder-core", "kaldi-native-fbank-core", "kissfft-float", "ssentencepiece_core", "onnxruntime"]
    subprocess.run(["/usr/bin/clang++", "-std=c++17", "-O2", "-Wall", "-Wextra", *extra_flags, "-I" + str(header.parent), str(ROOT / "scripts/diagnostics/sensevoice-memory.cc"), *[str(libs / ("lib" + n + ".a")) for n in names], "-framework", "Foundation", "-o", str(executable)], check=True)
    report = {"schema": 1, "completed": False, "os": platform.platform(), "machine": platform.machine(), "threads": 4, "duration_samples": 4800000, "audio_sha256": digest(audio), "model_sha256": digest(model_path), "candidate": candidate_evidence, "sherpa": lock["sherpa_version"], "ort": lock["onnxruntime_version"], "repeats": args.repeats, "cases": []}
    print(json.dumps({"report_directory": str(work)}), flush=True)
    for mode in args.modes:
        config = work / (mode + ".conf")
        options = MODES[mode]
        if mode == "node_stats":
            options += "SessionConfig.session.collect_node_memory_stats_to_file=" + str(work / "node-memory.csv") + "\n"
        config.write_text(options)
        provider = "cpu" if mode == "default" else "cpu:" + str(config)
        output = work / mode
        print(json.dumps({"running": mode}), flush=True)
        result = subprocess.run([str(executable), str(model_path), str(model_root / "tokens.txt"), str(audio), provider, str(output), str(args.repeats), mode], capture_output=True, text=True, timeout=300 * args.repeats)
        (work / (mode + ".stderr.txt")).write_text(result.stderr)
        (work / (mode + ".stdout.jsonl")).write_text(result.stdout)
        result.check_returncode()
        measurements = [json.loads(line) for line in result.stdout.splitlines()]
        transcripts = []
        for i in range(args.repeats):
            path = pathlib.Path(str(output) + "." + str(i))
            text = path.read_text()
            count = text.lower().count("tribal")
            require(count == 37 and "<|" not in text, "public long-form output failed phrase check")
            if candidate_evidence:
                require(digest(path) == "67658f46ee7ca478852d1770349001892c8665696429fd7931b64698940657cb", "candidate changed the baseline public transcript")
            transcripts.append({"sha256": digest(path), "bytes": path.stat().st_size, "phrase_count": count})
        case = {"mode": mode, "options": MODES[mode], "measurements": measurements, "transcripts": transcripts}
        if mode == "node_stats":
            with (work / "node-memory.csv").open() as source:
                rows = list(csv.DictReader(source))
            require(bool(rows), "runtime did not emit node memory statistics")
            case["node_stats"] = {"count": len(rows), "max_dynamic_bytes": max(int(r["total_dynamic_sizes"]) for r in rows), "max_temporary_bytes": max(int(r["total_temp_allocations"]) for r in rows), "csv_sha256": digest(work / "node-memory.csv")}
        report["cases"].append(case)
        (work / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(case), flush=True)
    hashes = {t["sha256"] for c in report["cases"] for t in c["transcripts"]}
    require(len(hashes) == 1, "allocation change altered the public fixture transcript")
    report["completed"] = True
    (work / "report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
