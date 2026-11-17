# SpeakMore binding patch 1

Source: crates.io `whisper-rs` 0.16.0, Unlicense, copied without its Cargo.lock.
The native engine remains the unmodified `whisper-rs-sys` 0.15.0 dependency,
whose vendored CMake project identifies itself as whisper.cpp 1.8.3.

`FullParams` language, initial prompt, and VAD-path setters previously leaked
their `CString::into_raw()` allocations. These three buffers now use owned
`Arc<CString>` fields. Parameter clones retain shared ownership; changing a
field affects only that parameter value, and the last owner frees the buffer.
The generated C types and function signatures are unchanged.

The two upstream microbenchmarks are gated behind `nightly-bench` so ordinary
unit tests, including the ownership regression, run on the stable toolchain.

The upstream safe abort setter casts a boxed trait object to its concrete
closure type. Safe callback setters also lose the allocations they create.
SpeakMore does not invoke those setters: `crates/asr-worker/src/cancellation.rs`
owns an exactly typed callback allocation for the synchronous full call. Native
callbacks only read an atomic, never write pipe output. We deliberately do not
claim all upstream APIs are audited; only the worker's used surface is covered.

Before upgrading, review and either carry or remove this patch based on source
inspection, run the owned-string tests and real worker cancellation tests, and
repeat ASR quality and compatibility gates. Do not replace the native engine
independently of the binding's dependency.
