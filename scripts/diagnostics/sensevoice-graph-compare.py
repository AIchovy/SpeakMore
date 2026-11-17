#!/usr/bin/env python3
"""Compare baseline and experimental graph using the exact product native library.
Only locked public files; one sequential recognizer/process per graph. Offline.
"""
import argparse
import importlib.util
import json
import pathlib
import platform
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("memory_diagnostic", pathlib.Path(__file__).with_name("sensevoice-memory.py"))
common = importlib.util.module_from_spec(spec)
spec.loader.exec_module(common)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("candidate_report", type=pathlib.Path)
    args = parser.parse_args()
    common.require(platform.system() == "Darwin" and platform.machine() == "arm64", "requires Apple Silicon macOS")
    path = args.candidate_report.absolute()
    common.require(path.is_file() and not path.is_symlink() and path.parent.parent == ROOT / ".cache/sensevoice/graph-study", "expected isolated generated candidate report")
    common.require(not any(p.is_symlink() for p in path.parents), "candidate directory is a symlink")
    candidate = json.loads(path.read_text())
    model_root = ROOT / ".cache/sensevoice/model"
    fixtures = json.loads((ROOT / "resources/sensevoice-fixtures.lock.json").read_text())
    audio_hashes = {}
    for f in fixtures["files"]:
        if f["filename"] in ("model.int8.onnx", "tokens.txt") or f["filename"].startswith("test_wavs/"):
            common.verify(model_root / f["filename"], f["size_bytes"], f["sha256"])
            audio_hashes[f["filename"]] = f["sha256"]
    common.require(candidate.get("completed") is True and candidate.get("source_sha256") == audio_hashes["model.int8.onnx"] and candidate.get("layers") == 70, "invalid candidate provenance")
    candidate_model = path.parent / "model.tiled.onnx"
    common.verify(candidate_model, candidate["candidate_bytes"], candidate["candidate_sha256"])
    lock = json.loads((ROOT / "resources/sensevoice-native.lock.json").read_text())
    libs = ROOT / ".cache/sensevoice/native" / lock["directory"] / "lib"
    header = ROOT / "vendor/sherpa-onnx/c-api.h"
    common.verify(header, lock["header"]["size_bytes"], lock["header"]["sha256"])
    for f in lock["libraries"]:
        common.verify(libs / f["filename"], f["size_bytes"], f["sha256"])
    work = pathlib.Path(tempfile.mkdtemp(prefix="compare-", dir=path.parent.parent))
    executable = work / "compare"
    names = ["sherpa-onnx-c-api", "sherpa-onnx-core", "sherpa-onnx-kaldifst-core", "sherpa-onnx-fstfar", "sherpa-onnx-fst", "kaldi-decoder-core", "kaldi-native-fbank-core", "kissfft-float", "ssentencepiece_core", "onnxruntime"]
    source = pathlib.Path(__file__).with_suffix(".cc")
    subprocess.run(["/usr/bin/clang++", "-std=c++17", "-O2", "-Wall", "-Wextra", "-I" + str(header.parent), str(source), *[str(libs / ("lib" + n + ".a")) for n in names], "-framework", "Foundation", "-o", str(executable)], check=True)
    report = {"schema": 1, "completed": False, "os": platform.platform(), "machine": platform.machine(), "threads": 4, "sherpa": lock["sherpa_version"], "ort": lock["onnxruntime_version"], "fixtures": audio_hashes, "candidate_sha256": candidate["candidate_sha256"], "candidate_report_sha256": common.digest(path), "native_source_sha256": common.digest(source), "runs": {}, "comparisons": []}
    print(json.dumps({"directory": str(work)}), flush=True)
    for name, model in [("baseline", model_root / "model.int8.onnx"), ("candidate", candidate_model)]:
        output = work / name
        output.mkdir()
        print(json.dumps({"running": name}), flush=True)
        result = subprocess.run([str(executable), str(model), str(model_root / "tokens.txt"), str(model_root), str(output)], capture_output=True, text=True, timeout=600)
        (output / "native.stdout.jsonl").write_text(result.stdout)
        (output / "native.stderr.txt").write_text(result.stderr)
        result.check_returncode()
        report["runs"][name] = [json.loads(line) for line in result.stdout.splitlines()]
        (work / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    baseline = sorted((work / "baseline").glob("*.json"))
    common.require(len(baseline) == 13 and len(list((work / "candidate").glob("*.json"))) == 13, "incomplete native suite")
    identical = True
    for file in baseline:
        left = json.loads(file.read_text())
        right_path = work / "candidate" / file.name
        right = json.loads(right_path.read_text())
        same = left == right
        identical = identical and same
        report["comparisons"].append({"case": file.stem, "all_result_fields_equal": same, "baseline_sha256": common.digest(file), "candidate_sha256": common.digest(right_path), "text_bytes": len(left["text"].encode()), "token_count": len(left.get("tokens", [])), "language": left.get("lang")})
    report["completed"] = identical
    (work / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report), flush=True)
    common.require(identical, "candidate result differs; inspect recorded comparison")


if __name__ == "__main__":
    main()
