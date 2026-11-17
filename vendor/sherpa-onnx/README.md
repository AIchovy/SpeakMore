# sherpa-onnx C API inputs

`c-api.h` and `LICENSE` are unmodified files from the upstream **v1.13.8** tag:
https://github.com/k2-fsa/sherpa-onnx/tree/v1.13.8

The header is verified against `resources/sensevoice-native.lock.json` before bindgen generates layouts. We do not handwrite parameter struct layouts. All native archives in that lock are checked for size and SHA-256 at build time. They are developer/build inputs only and are never downloaded by the product at runtime.

The first native target is Apple Silicon macOS. The no-TTS static release keeps Python, JNI, PortAudio and TTS out of the runtime boundary; only the C API and required decoding libraries are linked. ONNX Runtime and the other included dependencies require their own notices before commercial distribution; the sherpa-onnx Apache-2.0 license alone is not sufficient for the complete binary.

Model files have a separate license. The currently evaluated 2024-07-17 SenseVoice ONNX export points to the FunASR model license; do not relabel it MIT or Apache-2.0 based on the toolkit source license.
