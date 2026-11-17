//! Deliberately faulty subprocess for supervisor tests. Excluded from default
//! builds and never packaged as an application worker or used as ASR evidence.
use speakmore_protocol::{read_frame, write_control, Control, Frame, BUILD_ID, VERSION};
use std::{
    io::{self, Write},
    path::PathBuf,
    thread,
    time::Duration,
};

fn send(control: Control) {
    write_control(&mut io::stdout().lock(), &control).unwrap();
}
fn mode(root: &std::path::Path) -> String {
    std::fs::read_to_string(root.join("fixture-mode")).unwrap()
}
fn main() {
    let root: PathBuf = std::env::current_exe().unwrap().parent().unwrap().into();
    let mode = mode(&root);
    let counter_path = root.join("spawn-count");
    let count: u32 = std::fs::read_to_string(&counter_path)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    std::fs::write(&counter_path, (count + 1).to_string()).unwrap();
    std::fs::write(root.join("last-pid"), std::process::id().to_string()).unwrap();
    if mode == "crash" {
        std::process::exit(70);
    }
    if mode == "hello_stall" {
        thread::sleep(Duration::from_secs(60));
        return;
    }
    if mode == "huge_audio_output" {
        let mut out = io::stdout().lock();
        out.write_all(&(19_200_021u32).to_le_bytes()).unwrap();
        out.write_all(&[4]).unwrap();
        out.write_all(&[0; 16]).unwrap();
        out.write_all(&4_800_000u32.to_le_bytes()).unwrap();
        out.flush().unwrap();
        thread::sleep(Duration::from_secs(60));
        return;
    }
    let backend = if std::env::args().any(|s| s == "--cpu") {
        "cpu"
    } else {
        "metal"
    };
    let native_engine = if std::env::current_exe()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("sensevoice")
    {
        "sherpa-onnx"
    } else {
        "whisper.cpp"
    };
    send(Control::Hello {
        protocol_version: VERSION,
        engine_id: if mode == "wrong_engine" && native_engine == "sherpa-onnx" {
            "whisper.cpp"
        } else {
            native_engine
        }
        .into(),
        build_id: if mode == "bad_build" {
            "other-build"
        } else {
            BUILD_ID
        }
        .into(),
        backend: backend.into(),
    });
    let mut input = io::stdin().lock();
    let mut request = None;
    let mut previous: Option<Control> = None;
    loop {
        match read_frame(&mut input) {
            Ok(Frame::Control(Control::LoadModel {
                request_id,
                model_id,
                engine_id,
                profile_id,
                bundle_sha256,
                ..
            })) => {
                if mode == "model_stall" {
                    thread::sleep(Duration::from_secs(60));
                    return;
                }
                if (["gpu_fail", "gpu_unavailable"].contains(&mode.as_str()) && backend == "metal")
                    || (mode == "switch_fail" && model_id.contains("turbo"))
                    || (mode == "sensevoice_fail" && engine_id == "sherpa-onnx")
                    || (mode == "version_fail"
                        && std::fs::read_to_string(root.join("rejected-bundle"))
                            .is_ok_and(|value| value == bundle_sha256))
                {
                    send(Control::Failed {
                        session_id: None,
                        request_id: Some(request_id),
                        code: if mode == "gpu_unavailable" {
                            "GpuUnavailable"
                        } else {
                            "ModelLoadFailed"
                        }
                        .into(),
                    });
                } else {
                    send(Control::ModelReady {
                        request_id,
                        model_id,
                        backend: backend.into(),
                        engine_id,
                        profile_id,
                        bundle_sha256: if mode == "wrong_bundle" {
                            "3".repeat(64)
                        } else {
                            bundle_sha256
                        },
                    });
                }
                if mode == "ready_crash" {
                    thread::sleep(Duration::from_millis(100));
                    std::process::exit(70);
                }
            }
            Ok(Frame::Control(control @ Control::Transcribe { .. })) => {
                if mode == "transfer_stall" {
                    thread::sleep(Duration::from_secs(60));
                    return;
                }
                request = Some(control);
            }
            Ok(Frame::Audio {
                session_id,
                samples,
            }) => {
                let Control::Transcribe {
                    dictionary_revision,
                    dictionary_terms,
                    ..
                } = request.take().unwrap()
                else {
                    unreachable!()
                };
                if mode != "before_ack" {
                    send(Control::InputAccepted {
                        session_id,
                        sample_count: samples.len() as u32 + u32::from(mode == "wrong_count"),
                    });
                }
                drop(samples);
                if mode == "decode_stall" {
                    thread::sleep(Duration::from_secs(60));
                    return;
                }
                if mode == "decode_crash" {
                    std::process::exit(70);
                }
                if ["cooperative_cancel", "cancel_exit"].contains(&mode.as_str()) {
                    continue;
                }
                if mode == "late_completed" {
                    // Wait for Cancel, then deliberately send a successful result.
                    let _ = read_frame(&mut input);
                }
                if mode == "replay_old" {
                    if let Some(old) = previous.take() {
                        send(old);
                    }
                }
                if mode == "progress_flood" {
                    for _ in 0..1000 {
                        send(Control::Progress {
                            session_id,
                            stage: "decoding".into(),
                            progress: None,
                        });
                    }
                }
                let complete = Control::Completed {
                    session_id,
                    text: "Synthetic test result".into(),
                    language: "en".into(),
                    asr_ms: 1,
                    dictionary_revision: dictionary_revision + i64::from(mode == "wrong_revision"),
                    prompt_terms_used: dictionary_terms.len() as u32,
                    count_omitted: 0,
                    quality_flags: if mode == "needs_review" {
                        vec![speakmore_protocol::QualityFlag::NeedsReview]
                    } else {
                        vec![]
                    },
                };
                previous = Some(complete.clone());
                send(complete);
            }
            Ok(Frame::Control(Control::Cancel { session_id })) => {
                if mode == "cancel_exit" {
                    // Like the real worker's watchdog for non-interruptible native work.
                    std::process::exit(75);
                }
                send(Control::Cancelled { session_id });
            }
            Ok(Frame::Control(Control::Shutdown { .. })) | Err(_) => return,
            _ => std::process::exit(64),
        }
    }
}
