"""Freeze a deterministic diagnostic subset before any inference is run."""
import csv
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import re
import sys
import tarfile

import numpy as np
import pyarrow.parquet as pq
import soundfile as sf

ROOT = Path(__file__).resolve().parents[2]
CACHE = ROOT / ".cache/asr-quality"
SEED = "speakmore-public-development-v1"
COUNT = 100


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def rank(identifier):
    return sha((SEED + "\0" + identifier).encode())


def main():
    output = CACHE / "corpus-v1"
    if any(p.is_symlink() for p in [output, *output.parents]):
        raise ValueError("Symlink in output path")
    output.mkdir()  # Never change a previous manifest or overwrite its audio.
    (output / "audio").mkdir()
    lock_bytes = (ROOT / "resources/asr-quality-sources.lock.json").read_bytes()
    lock = json.loads(lock_bytes)
    module_spec = importlib.util.spec_from_file_location(
        "quality_download", Path(__file__).with_name("download-quality-corpus.py"))
    download = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(download)
    for file in lock["files"]:
        if not download.verify(CACHE / "downloads" / file["filename"], file):
            raise ValueError("Source hash does not match lock")
    clips = []
    pools = []

    def save(identifier, raw_audio, text, language, dataset, revision,
             speaker, source_samples=None, source_id=None):
        source = io.BytesIO(raw_audio)
        data, rate = sf.read(source, dtype="float64", always_2d=True)
        if rate != 16000 or data.shape[1] != 1 or not np.isfinite(data).all():
            raise ValueError("Unsupported source audio; never silently resample or truncate")
        if not 4000 <= len(data) <= 4_800_000:
            raise ValueError("Selected clip is outside supported duration")
        if source_samples is not None and len(data) != source_samples:
            raise ValueError("Source metadata sample count mismatch")
        wav = io.BytesIO()
        sf.write(wav, data, rate, format="WAV", subtype="PCM_16")
        audio = wav.getvalue()
        relative = "audio/" + sha(identifier.encode()) + ".wav"
        with (output / relative).open("xb") as stream:
            stream.write(audio)
        clips.append(dict(id=identifier, language=language, dataset=dataset,
                          revision=revision, source_id=source_id, speaker=speaker,
                          reference=text, critical_fragments=None,
                          samples=len(data), wav=relative, wav_sha256=sha(audio),
                          source_audio_sha256=sha(raw_audio),
                          conversion="16k mono to PCM16; no trimming, resampling or concatenation",
                          clipped_source_samples=int(np.sum(np.abs(data) > 1))))

    for lang, group in [("cmn_hans_cn", "zh"), ("en_us", "en")]:
        record = next(f for f in lock["files"] if f["language"] == lang and f["filename"].endswith(".tsv"))
        revision = record["revision"]
        with (CACHE / "downloads" / record["filename"]).open(newline="") as f:
            rows = list(csv.reader(f, delimiter="\t"))
        # FLEURS repeats prompts across speakers. Pick only one recording per
        # prompt, by the same fixed hash order; no model-dependent selection.
        ordered = sorted(rows, key=lambda r: rank(f"google/fleurs/{revision}/{lang}/{r[1]}"))
        selected = []
        seen_prompts = set()
        invalid = 0
        for row in ordered:
            if len(row) != 7 or not row[2].strip() or not 4000 <= int(row[5]) <= 4_800_000:
                invalid += 1
                continue
            if row[0] not in seen_prompts:
                selected.append(row)
                seen_prompts.add(row[0])
        pools.append(dict(dataset="google/fleurs", language=group, total=len(rows),
                          unique_eligible_prompts=len(selected), invalid=invalid,
                          selected=COUNT, speaker_ids="unavailable in pinned TSV"))
        if len(selected) < COUNT:
            raise ValueError("Not enough unique prompts")
        selected = selected[:COUNT]
        wanted = {"dev/" + r[1]: r for r in selected}
        found = {}
        with tarfile.open(CACHE / "downloads" / f"fleurs-{lang}-dev.tar.gz", "r:gz") as archive:
            for member in archive:
                if member.name in wanted:
                    if not member.isfile() or member.size > 40_000_000 or member.name in found:
                        raise ValueError("Unexpected archive member")
                    found[member.name] = archive.extractfile(member).read()
        if found.keys() != wanted.keys():
            raise ValueError("Missing selected source audio")
        for row in selected:
            save(f"fleurs/{lang}/{row[1]}", found["dev/" + row[1]], row[2], group,
                 "google/fleurs", revision, None, int(row[5]), row[0])

    record = next(f for f in lock["files"] if f["dataset"] == "CAiRE/ASCEND")
    rows = pq.read_table(CACHE / "downloads" / record["filename"]).to_pylist()
    eligible = [r for r in rows if r["language"] == "mixed"
                and re.search(r"[\u3400-\u9fff]", r["transcription"])
                and re.search(r"[A-Za-z]", r["transcription"])
                and .25 <= r["duration"] <= 300]
    eligible.sort(key=lambda r: rank(f"CAiRE/ASCEND/{record['revision']}/{r['id']}"))
    if len(eligible) < COUNT:
        raise ValueError("Not enough bilingual clips")
    pools.append(dict(dataset="CAiRE/ASCEND", language="mixed", total=len(rows),
                      labeled_mixed=sum(r["language"] == "mixed" for r in rows),
                      eligible_bilingual=len(eligible), selected=COUNT))
    for row in eligible[:COUNT]:
        save("ascend/validation/" + row["id"], row["audio"]["bytes"], row["transcription"],
             "mixed", "CAiRE/ASCEND", record["revision"], row["original_speaker_id"],
             source_id=row["id"])
    if len({c["wav_sha256"] for c in clips}) != len(clips):
        raise ValueError("Duplicate selected audio")
    manifest = dict(schema=1, purpose="public_development_diagnostic", seed=SEED,
                    source_lock_sha256=sha(lock_bytes), sources=lock, pools=pools, clips=clips,
                    selection="SHA256 rank; FLEURS unique prompts; ASCEND labeled mixed with Latin and CJK",
                    limitations=["Public development splits; not an independent acceptance set",
                                 "No recorded consent for a private SpeakMore acceptance corpus",
                                 "FLEURS speaker identities unavailable; speaker separation unverified",
                                 "No long-duration or negative coverage required by release gate",
                                 "Critical fragments not annotated; exact accuracy not measured"])
    target = output / "manifest.json"
    with target.open("x") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2)
        f.write("\n")
    print(json.dumps({"manifest":str(target), "sha256":sha(target.read_bytes()),
                      "clips":len(clips), "pools":pools}, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
