#!/usr/bin/env python3
"""Developer-only, offline graph experiment. Never changes a trusted product model.

Tile query rows while keeping every key/value row and both original masks.
The same complete waveform still goes through one native decode invocation.
Requires ONNX 1.17.0 in the project-local diagnostic Python target.
"""
import argparse
import collections
import copy
import hashlib
import json
import pathlib
import tempfile

import numpy as np
import onnx
from onnx import TensorProto as T, helper as H, numpy_helper as N

ROOT = pathlib.Path(__file__).resolve().parents[2]
SOURCE_SIZE = 239233841
SOURCE_SHA = "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def tiled_attention(prefix, q, kt, v, mask, masked_value, zero_mask, zero, output, block):
    """Return nodes and constants for [B,H,T,D] attention, padded queries only.

    Masks broadcast as [B,1,1,K]. Keys/values are never padded or partitioned.
    Each Loop scan result has a fixed query-block size, including the last one.
    """
    constants = []
    nodes = []

    def name(value):
        return prefix + "/" + value

    def const(key, value, dtype=np.int64):
        constants.append(N.from_array(np.asarray(value, dtype=dtype), name(key)))
        return name(key)

    def op(kind, inputs, result, **attrs):
        nodes.append(H.make_node(kind, inputs, [name(result)], name=name(result), **attrs))
        return name(result)

    block_scalar = const("block", block)
    block_minus_one = const("block_minus_one", block - 1)
    axis = const("query_axis", [2])
    axis_scalar = const("query_axis_scalar", 2)
    zero_vec = const("zero_vec", [0])
    zero_scalar = const("zero_float", 0, np.float32)
    true = const("true", True, np.bool_)
    shape = op("Shape", [q], "query_shape")
    length = op("Gather", [shape, axis_scalar], "length", axis=0)
    rounded = op("Add", [length, block_minus_one], "rounded")
    trips = op("Div", [rounded, block_scalar], "trips")
    padded_length = op("Mul", [trips, block_scalar], "padded_length")
    padding = op("Sub", [padded_length, length], "padding")
    pad_vec = op("Unsqueeze", [padding, zero_vec], "pad_vec")
    pads = op("Concat", [const("pad_prefix", [0] * 6), pad_vec, zero_vec], "pads", axis=0)
    padded_query = op("Pad", [q, pads, zero_scalar], "padded_query", mode="constant")

    # The body uses the outer scope's original key/value tensors and masks.
    body = []

    def inner(kind, inputs, result, **attrs):
        body.append(H.make_node(kind, inputs, [name(result)], name=name(result), **attrs))
        return name(result)

    iteration = name("iteration")
    condition = name("condition")
    begin = inner("Mul", [iteration, block_scalar], "begin")
    end = inner("Add", [begin, block_scalar], "end")
    starts = inner("Unsqueeze", [begin, zero_vec], "starts")
    ends = inner("Unsqueeze", [end, zero_vec], "ends")
    query = inner("Slice", [padded_query, starts, ends, axis], "query_block")
    scores = inner("MatMul", [query, kt], "scores")
    masked = inner("Where", [mask, masked_value, scores], "masked")
    probabilities = inner("Softmax", [masked], "probabilities", axis=-1)
    masked_probabilities = inner("Where", [zero_mask, zero, probabilities], "masked_probabilities")
    result = inner("MatMul", [masked_probabilities, v], "result")
    condition_out = inner("Identity", [condition], "condition_out")
    graph = H.make_graph(body, name("body"), [
        H.make_tensor_value_info(iteration, T.INT64, []),
        H.make_tensor_value_info(condition, T.BOOL, []),
    ], [
        H.make_tensor_value_info(condition_out, T.BOOL, []),
        H.make_tensor_value_info(result, T.FLOAT, [None, None, block, None]),
    ])
    scan = op("Loop", [trips, true], "scan", body=graph)
    transposed = op("Transpose", [scan], "transposed", perm=[1, 2, 0, 3, 4])
    leading = op("Gather", [shape, const("leading_indices", [0, 1])], "leading", axis=0)
    width = op("Gather", [shape, const("width_index", [3])], "width", axis=0)
    result_shape = op("Concat", [leading, const("infer_length", [-1]), width], "result_shape", axis=0)
    joined = op("Reshape", [transposed, result_shape], "joined")
    length_vec = op("Unsqueeze", [length, zero_vec], "length_vec")
    nodes.append(H.make_node("Slice", [joined, zero_vec, length_vec, axis], [output], name=name("trim_queries")))
    return nodes, constants


def rewrite(model, block):
    require(model.ir_version == 7 and [(v.domain, v.version) for v in model.opset_import] == [("", 13)], "unexpected source format")
    nodes = list(model.graph.node)
    producers = {o: n for n in nodes for o in n.output}
    consumers = collections.defaultdict(list)
    for n in nodes:
        for i in n.input:
            consumers[i].append(n)
    replacements = {}
    remove = set()
    constants = []
    count = 0
    for softmax in nodes:
        if softmax.op_type != "Softmax":
            continue
        require(len(softmax.attribute) == 1 and softmax.attribute[0].name == "axis" and softmax.attribute[0].i == -1, "unexpected softmax axis")
        masked = producers[softmax.input[0]]
        scores = producers[masked.input[2]]
        require(len(consumers[softmax.output[0]]) == 1, "shared probabilities")
        zeroed = consumers[softmax.output[0]][0]
        require(len(consumers[zeroed.output[0]]) == 1, "shared masked probabilities")
        result = consumers[zeroed.output[0]][0]
        require([n.op_type for n in (scores, masked, zeroed, result)] == ["MatMul", "Where", "Where", "MatMul"], "unexpected attention structure")
        require(zeroed.input[2] == softmax.output[0] and result.input[0] == zeroed.output[0], "unexpected attention operands")
        for n in (scores, masked, softmax, zeroed):
            require(len(n.output) == 1 and len(consumers[n.output[0]]) == 1, "attention intermediate escapes")
            require(n.output[0] not in [o.name for o in model.graph.output], "intermediate is a model output")
        tiled, init = tiled_attention("speakmore_query_tile_" + str(count), scores.input[0], scores.input[1], result.input[1], masked.input[0], masked.input[1], zeroed.input[0], zeroed.input[1], result.output[0], block)
        replacements[result.name] = tiled
        remove.update(n.name for n in (scores, masked, softmax, zeroed, result))
        constants.extend(init)
        count += 1
    require(count == 70, "expected all 70 attention layers")
    output = []
    for n in nodes:
        if n.name in replacements:
            output.extend(replacements[n.name])
        elif n.name not in remove:
            output.append(n)
    del model.graph.node[:]
    model.graph.node.extend(output)
    model.graph.initializer.extend(constants)
    onnx.checker.check_model(model)
    return count


def self_test(block):
    # Unit algebra check only. Full native checks use the product's pinned ORT.
    import onnxruntime as ort
    inputs = [H.make_tensor_value_info(n, T.BOOL if n == "mask" else T.FLOAT, s) for n, s in [
        ("q", [None, 4, None, 128]), ("kt", [None, 4, 128, None]),
        ("v", [None, 4, None, 128]), ("mask", [None, 1, 1, None]),
    ]]
    base = [H.make_node("MatMul", ["q", "kt"], ["scores"]),
            H.make_node("Where", ["mask", "negative", "scores"], ["masked"]),
            H.make_node("Softmax", ["masked"], ["probabilities"], axis=-1),
            H.make_node("Where", ["mask", "zero", "probabilities"], ["zeroed"]),
            H.make_node("MatMul", ["zeroed", "v"], ["output"])]
    tiled, constants = tiled_attention("test", "q", "kt", "v", "mask", "negative", "mask", "zero", "output", block)
    shared = [N.from_array(np.asarray(-np.inf, dtype=np.float32), "negative"), N.from_array(np.asarray(0, dtype=np.float32), "zero")]
    sessions = []
    for name, nodes, init in [("reference", base, shared), ("tiled", tiled, shared + constants)]:
        model = H.make_model(H.make_graph(nodes, name, inputs, [H.make_tensor_value_info("output", T.FLOAT, [None, 4, None, 128])], init), opset_imports=[H.make_opsetid("", 13)], ir_version=7)
        onnx.checker.check_model(model)
        options = ort.SessionOptions()
        options.intra_op_num_threads = 4
        sessions.append(ort.InferenceSession(model.SerializeToString(), options, providers=["CPUExecutionProvider"]))
    rng = np.random.default_rng(20260916)
    cases = []
    for length in (1, block - 1, block, block + 1, 2 * block + 7):
        for batch, all_masked in [(1, False), (2, False), (1, True)]:
            data = {
                "q": (rng.normal(size=(batch, 4, length, 128)) / np.sqrt(128)).astype(np.float32),
                "kt": rng.normal(size=(batch, 4, 128, length)).astype(np.float32),
                "v": rng.normal(size=(batch, 4, length, 128)).astype(np.float32),
                "mask": rng.random((batch, 1, 1, length)) < .2,
            }
            data["mask"][:, :, :, 0] = False
            if all_masked:
                data["mask"].fill(True)
            reference, candidate = [s.run(None, data)[0] for s in sessions]
            require(reference.shape == candidate.shape, "query padding changed output shape")
            require(np.isfinite(candidate).all(), "non-finite attention result")
            np.testing.assert_allclose(candidate, reference, rtol=1e-5, atol=1e-6)
            cases.append({"length": length, "batch": batch, "all_masked": all_masked, "max_abs_difference": float(np.max(np.abs(candidate - reference)))})
    return {"ort": ort.__version__, "numpy": np.__version__, "block": block, "cases": cases}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--block", type=int, choices=(64, 128, 256), default=128)
    args = parser.parse_args()
    require(onnx.__version__ == "1.17.0", "requires diagnostic ONNX 1.17.0")
    source = ROOT / ".cache/sensevoice/model/model.int8.onnx"
    require(source.is_file() and not source.is_symlink() and source.stat().st_size == SOURCE_SIZE and digest(source) == SOURCE_SHA, "source model differs from pinned public artifact")
    parent = ROOT / ".cache/sensevoice/graph-study"
    require(not any(p.is_symlink() for p in [parent, *parent.parents]), "symlink output parent")
    parent.mkdir(parents=True, exist_ok=True)
    work = pathlib.Path(tempfile.mkdtemp(prefix="tiled-", dir=parent))
    report = {"schema": 1, "completed": False, "source_sha256": SOURCE_SHA, "onnx": onnx.__version__, "block": args.block, "algebra": self_test(args.block)}
    model = onnx.load(source)
    original_initializers = {x.name: hashlib.sha256(x.SerializeToString()).hexdigest() for x in model.graph.initializer}
    original_metadata = copy.deepcopy(model.metadata_props)
    report["layers"] = rewrite(model, args.block)
    require(model.metadata_props == original_metadata, "metadata changed")
    after = {x.name: hashlib.sha256(x.SerializeToString()).hexdigest() for x in model.graph.initializer}
    require(all(after[n] == h for n, h in original_initializers.items()), "source initializer changed")
    candidate = work / "model.tiled.onnx"
    onnx.save(model, candidate)
    report["candidate_sha256"] = digest(candidate)
    report["candidate_bytes"] = candidate.stat().st_size
    report["completed"] = True
    (work / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"directory": str(work), **report}), flush=True)


if __name__ == "__main__":
    main()
