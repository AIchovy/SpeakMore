use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

fn verify(path: &std::path::Path, record: &serde_json::Value) {
    let metadata = fs::symlink_metadata(path).unwrap_or_else(|_| {
        panic!("Missing SenseVoice native inputs. Run python3 scripts/prepare-sensevoice-native.py")
    });
    assert!(metadata.is_file(), "Native inputs must be regular files");
    let raw = fs::read(path).expect("read native input");
    assert_eq!(
        raw.len() as u64,
        record["size_bytes"].as_u64().unwrap(),
        "Native input size mismatch"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(raw)),
        record["sha256"].as_str().unwrap(),
        "Native input hash mismatch"
    );
    println!("cargo:rerun-if-changed={}", path.display());
}
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("TARGET").unwrap() != "aarch64-apple-darwin" {
        return;
    }
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let lock = root.join("resources/sensevoice-native.lock.json");
    println!("cargo:rerun-if-changed={}", lock.display());
    let record: serde_json::Value = serde_json::from_slice(&fs::read(lock).unwrap()).unwrap();
    println!(
        "cargo:rustc-env=SPEAKMORE_SHERPA_VERSION={}",
        record["sherpa_version"].as_str().unwrap()
    );
    println!(
        "cargo:rustc-env=SPEAKMORE_ORT_VERSION={}",
        record["onnxruntime_version"].as_str().unwrap()
    );
    let header = root.join("vendor/sherpa-onnx/c-api.h");
    verify(&header, &record["header"]);
    let libs = root
        .join(".cache/sensevoice/native")
        .join(record["directory"].as_str().unwrap())
        .join("lib");
    for library in record["libraries"].as_array().unwrap() {
        verify(&libs.join(library["filename"].as_str().unwrap()), library);
    }
    let bindings = bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .allowlist_function("SherpaOnnx(GetVersionStr|GetOnnxruntimeVersionStr|CreateOfflineRecognizer|DestroyOfflineRecognizer|OfflineRecognizerSetConfig|CreateOfflineStream|DestroyOfflineStream|AcceptWaveformOffline|DecodeOfflineStream|GetOfflineStreamResult|DestroyOfflineRecognizerResult|CreateVoiceActivityDetector|DestroyVoiceActivityDetector|VoiceActivityDetectorAcceptWaveform|VoiceActivityDetectorEmpty|VoiceActivityDetectorFront|VoiceActivityDetectorPop|VoiceActivityDetectorReset|VoiceActivityDetectorFlush|DestroySpeechSegment)")
        .derive_default(true)
        .generate_comments(false)
        .layout_tests(true)
        .generate().expect("Generate bindings from pinned C API header");
    bindings
        .write_to_file(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("bindings.rs"))
        .unwrap();
    println!("cargo:rustc-link-search=native={}", libs.display());
    for library in [
        "sherpa-onnx-c-api",
        "sherpa-onnx-core",
        "sherpa-onnx-kaldifst-core",
        "sherpa-onnx-fstfar",
        "sherpa-onnx-fst",
        "kaldi-decoder-core",
        "kaldi-native-fbank-core",
        "kissfft-float",
        "ssentencepiece_core",
        "onnxruntime",
    ] {
        println!("cargo:rustc-link-lib=static={library}");
    }
    println!("cargo:rustc-link-lib=c++");
    println!("cargo:rustc-link-lib=framework=Foundation");
}
