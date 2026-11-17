"""Versioned offline CER/WER/MER diagnostic. No model or threshold tuning.

NFKC + casefold, explicit punctuation rules, no number expansion, stemming,
traditional/simplified conversion or deletion of negation. Failed clips remain
in the denominator with an empty hypothesis. Incomplete runs are rejected.
"""
import argparse
from collections import Counter
import hashlib
import json
import math
from pathlib import Path
import unicodedata

SCORER = "speakmore-strict-text-v1"
# Decimal points, signs, percent/currency, @ / _ - and in-word apostrophes
# are retained. This is not a literal URL/critical-fragment exact-match metric.
# Punctuation removal creates word boundaries.
IGNORED = frozenset(',，。!！?？;；、()（）[]【】{}「」『』"“”《》<>…')


def is_han(char):
    return ('\u3400' <= char <= '\u9fff' or '\uf900' <= char <= '\ufaff'
            or '\U00020000' <= char <= '\U0003134f')


def normalize(text):
    text = unicodedata.normalize("NFKC", text).casefold().replace('’', "'").replace('‘', "'")
    output = []
    for i, char in enumerate(text):
        internal = i > 0 and i + 1 < len(text) and text[i-1].isalnum() and text[i+1].isalnum()
        colon_in_value = char == ':' and (internal or text[i:i+3] == '://')
        if char in IGNORED or (char in ".'" and not internal) or (char == ':' and not colon_in_value):
            output.append(" ")
        else:
            output.append(char)
    return " ".join("".join(output).split())


def tokens(text, language):
    normal = normalize(text)
    if language == "zh":
        return [c for c in normal if not c.isspace()]
    if language == "en":
        return normal.split()
    if language != "mixed":
        raise ValueError("Unknown metric language")
    result, word = [], []
    for char in normal:
        if char.isspace() or is_han(char):
            if word:
                result.append("".join(word))
                word = []
            if is_han(char):
                result.append(char)
        else:
            word.append(char)
    if word:
        result.append("".join(word))
    return result


def edits(reference, hypothesis):
    # Tuple: total, substitutions, deletions, insertions. Stable equal-distance
    # ties prefer diagonal, then deletion, then insertion; O(hypothesis) space.
    previous = [(j, 0, 0, j) for j in range(len(hypothesis) + 1)]
    for i, ref in enumerate(reference, 1):
        row = [(i, 0, i, 0)]
        for j, hyp in enumerate(hypothesis, 1):
            diagonal, deletion, insertion = previous[j-1], previous[j], row[j-1]
            different = int(ref != hyp)
            candidates = [(diagonal[0]+different, diagonal[1]+different, diagonal[2], diagonal[3]),
                          (deletion[0]+1, deletion[1], deletion[2]+1, deletion[3]),
                          (insertion[0]+1, insertion[1], insertion[2], insertion[3]+1)]
            row.append(min(candidates, key=lambda value: value[0]))
        previous = row
    total, substitutions, deletions, insertions = previous[-1]
    return dict(errors=total, substitutions=substitutions, deletions=deletions,
                insertions=insertions, reference_tokens=len(reference))


def quantiles(values):
    if not values:
        return None
    values = sorted(values)
    return dict(n=len(values), p50=values[math.ceil(.5*len(values))-1],
                p95=values[math.ceil(.95*len(values))-1], maximum=values[-1],
                method="nearest-rank", at_least_30_observations=len(values) >= 30)


def duration_bucket(seconds):
    for ceiling, label in [(5, "0.25–5s"), (15, ">5–15s"), (45, ">15–45s"),
                           (90, ">45–90s"), (240, ">90–240s"), (300, ">240–300s")]:
        if seconds <= ceiling:
            return label
    raise ValueError("Unsupported duration")


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def score(manifest_raw, results_raw):
    manifest = json.loads(manifest_raw)
    rows = [json.loads(line) for line in results_raw.splitlines()]
    if not rows or rows[0].get("type") != "header" or rows[-1] != {"type":"complete"}:
        raise ValueError("Incomplete diagnostic run")
    header = rows[0]
    if not header["models"] or len(set(header["models"])) != len(header["models"]):
        raise ValueError("Missing or duplicate model IDs")
    if header["manifest_sha256"] != sha(manifest_raw):
        raise ValueError("Manifest differs from inference run")
    if manifest["purpose"] != "public_development_diagnostic":
        raise ValueError("This scorer is not a release qualification")
    clips = {clip["id"]:clip for clip in manifest["clips"]}
    if len(clips) != len(manifest["clips"]) or len(clips) != header["clips"]:
        raise ValueError("Duplicate clip IDs or incorrect count")
    if not clips or any(c["language"] not in ("zh", "en", "mixed")
                        or not 4000 <= c["samples"] <= 4_800_000 for c in clips.values()):
        raise ValueError("Unsupported corpus language or duration")
    expected = {(model, identifier) for model in header["models"] for identifier in clips}
    results = {}
    for row in rows[1:-1]:
        if row["type"] == "model":
            continue
        if row["type"] != "result":
            raise ValueError("Unexpected record type")
        key = (row["model_id"], row["id"])
        if key not in expected or key in results:
            raise ValueError("Unknown or duplicate result")
        if row["status"] not in ("completed", "failed", "cancelled"):
            raise ValueError("Unknown result status")
        if row["status"] != "completed" and row["text"]:
            raise ValueError("Failed result must not contain a hypothesis")
        if row["status"] == "completed" and row["accepted_events"] != 1:
            raise ValueError("Completed input lacks exactly one acceptance event")
        if row["wall_ms"] < 0:
            raise ValueError("Negative duration")
        results[key] = row
    if results.keys() != expected:
        raise ValueError("Missing results; failures must remain in the denominator")
    summaries = []
    per_clip = []
    for model in header["models"]:
        for language in ["zh", "en", "mixed"]:
            group = [c for c in clips.values() if c["language"] == language]
            if not group:
                continue
            counts = Counter()
            flags, statuses, failures, buckets = Counter(), Counter(), Counter(), {}
            latency, rtf, all_attempts = [], [], []
            exact = 0
            for clip in group:
                result = results[(model, clip["id"])]
                ref = tokens(clip["reference"], language)
                if not ref:
                    raise ValueError("Positive speech reference is empty; negatives need a separate suite")
                hyp = tokens(result["text"], language)
                detail = edits(ref, hyp)
                counts.update(detail)
                exact += ref == hyp
                seconds = clip["samples"] / 16000
                bucket = duration_bucket(seconds)
                buckets.setdefault(bucket, {"attempted":0, "wall_ms":[], "rtf":[]})["attempted"] += 1
                statuses[result["status"]] += 1
                flags.update(result.get("quality_flags", []))
                all_attempts.append(result["wall_ms"])
                if result["status"] == "completed":
                    factor = result["wall_ms"] / (1000 * seconds)
                    latency.append(result["wall_ms"])
                    rtf.append(factor)
                    buckets[bucket]["wall_ms"].append(result["wall_ms"])
                    buckets[bucket]["rtf"].append(factor)
                else:
                    failures[result.get("error", result["status"])] += 1
                per_clip.append(dict(model_id=model, id=clip["id"], language=language,
                                     status=result["status"], **detail))
            summaries.append(dict(model_id=model, language=language, clips=len(group),
                metric={"zh":"CER", "en":"WER", "mixed":"MER"}[language],
                **counts, error_rate=counts["errors"]/counts["reference_tokens"],
                exact_utterances=exact, statuses=dict(statuses), failures=dict(failures),
                quality_flags=dict(flags), completed_wall_ms=quantiles(latency),
                completed_rtf=quantiles(rtf), all_attempt_wall_ms=quantiles(all_attempts),
                duration_buckets={k:dict(attempted=v["attempted"], completed_wall_ms=quantiles(v["wall_ms"]),
                                        completed_rtf=quantiles(v["rtf"])) for k,v in buckets.items()}))
    return dict(schema=1, scorer=SCORER, scorer_sha256=sha(Path(__file__).read_bytes()),
                manifest_sha256=sha(manifest_raw), results_sha256=sha(results_raw),
                purpose=manifest["purpose"], release_gate_passed=False,
                limits=manifest["limitations"], critical_fragment_accuracy=None,
                negative_automatic_input_tested=False, header=header, summaries=summaries,
                corpus_duration_seconds=sum(c["samples"] for c in clips.values())/16000,
                corpus_duration_range_seconds=[min(c["samples"] for c in clips.values())/16000,
                                               max(c["samples"] for c in clips.values())/16000],
                identifiable_ascend_speakers=sorted({c["speaker"] for c in clips.values() if c["speaker"] is not None}),
                per_clip=per_clip)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest", type=Path)
    parser.add_argument("results", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    report = score(args.manifest.read_bytes(), args.results.read_bytes())
    with args.output.open("x") as target:
        json.dump(report, target, ensure_ascii=False, indent=2)
        target.write("\n")
    for row in report["summaries"]:
        print(f"{row['model_id']} {row['language']} {row['metric']}={row['error_rate']:.4%} "
              f"({row['errors']}/{row['reference_tokens']}); {row['statuses']}")
