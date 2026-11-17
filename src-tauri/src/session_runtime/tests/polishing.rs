use super::*;
use speakmore_core::processing::Reason;
use std::io::{Read, Write};

// Exactly one fixed probe and one synthetic transcript. The second response is
// held until the test has inspected the raw cache, database and usage event.
struct Server {
    endpoint: String,
    received: tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
    release: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}
impl Server {
    fn new(response: &'static str) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/api/chat", listener.local_addr().unwrap());
        let (tx, received) = tokio::sync::mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            for step in 0..2 {
                let until = Instant::now() + Duration::from_secs(15);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < until =>
                        {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(error) => panic!("synthetic server: {error}"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                let body = loop {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    assert!(bytes.len() < 32_768);
                    if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length: usize = head
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .unwrap()
                            .trim()
                            .parse()
                            .unwrap();
                        assert!(!head.contains("authorization"));
                        if bytes.len() >= end + 4 + length {
                            break serde_json::from_slice::<serde_json::Value>(
                                &bytes[end + 4..end + 4 + length],
                            )
                            .unwrap();
                        }
                    }
                };
                if step == 0 {
                    assert_eq!(
                        body["messages"][1]["content"],
                        "Please add punctuation to this sentence"
                    );
                } else {
                    assert_eq!(body["messages"][1]["content"], "Synthetic test result");
                    tx.send(body).unwrap();
                    gate.recv_timeout(Duration::from_secs(8)).unwrap();
                }
                let text = if step == 0 {
                    "Please add punctuation to this sentence."
                } else {
                    response
                };
                let data = serde_json::to_vec(&serde_json::json!({"message":{"role":"assistant","content":text},"done":true,"done_reason":"stop"})).unwrap();
                let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len());
                let sent = socket
                    .write_all(header.as_bytes())
                    .and_then(|_| socket.write_all(&data));
                if step == 0 {
                    sent.unwrap();
                }
                let _ = socket.shutdown(std::net::Shutdown::Write);
            }
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock,
                "a session must not retry or submit twice"
            );
        });
        Self {
            endpoint,
            received,
            release,
            thread,
        }
    }
    async fn request(&mut self) -> serde_json::Value {
        tokio::time::timeout(Duration::from_secs(8), self.received.recv())
            .await
            .unwrap()
            .unwrap()
    }
}

async fn setup(server: &Server) -> (Harness, crate::providers::Service) {
    setup_mode(server, "normal").await
}
async fn setup_mode(server: &Server, mode: &str) -> (Harness, crate::providers::Service) {
    let h = Harness::new(mode, vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let (service, _) = crate::providers::Service::fixture(h.handle.clone());
    assert_eq!(
        service
            .enable_polishing(h.handle.snapshot().unwrap().settings.revision)
            .await
            .unwrap_err()
            .code,
        "ProviderNotTested"
    );
    let id = Uuid::new_v4();
    service.save(serde_json::from_value(serde_json::json!({
        "expectedRevision":h.handle.snapshot().unwrap().settings.revision,
        "providerId":id,"providerRevision":null,"displayName":"Synthetic refinement",
        "configuration":{"protocol":{"kind":"ollama"},"endpoint":server.endpoint,"model":"fixture"},
        "credential":{"action":"remove"},"confirmDestination":false
    })).unwrap()).await.unwrap();
    service.test(id, 1).await.unwrap();
    service
        .select(h.handle.snapshot().unwrap().settings.revision, Some(id))
        .await
        .unwrap();
    service
        .enable_polishing(h.handle.snapshot().unwrap().settings.revision)
        .await
        .unwrap();
    patch(
        &h,
        SettingsPatch {
            history_enabled: Some(true),
            ..Default::default()
        },
    )
    .await;
    (h, service)
}

#[tokio::test]
async fn review_flagged_dictation_uses_saved_default_prompt_and_preserves_literal_suffix() {
    let mut server = Server::new("Synthetic test result.\n—clean");
    let (h, _service) = setup_mode(&server, "needs_review").await;
    let preference = "Keep my voice, use email body paragraphs, end with —clean";
    patch(
        &h,
        SettingsPatch {
            polish_mode: Some("custom".into()),
            custom_prompt: Some(preference.into()),
            ..Default::default()
        },
    )
    .await;
    let id = h.record().await;
    h.handle.stop(id, false).await.unwrap();
    let request = server.request().await;
    let instructions = request["messages"][0]["content"].as_str().unwrap();
    assert!(instructions.contains(preference));
    assert!(instructions.contains("may be added only when explicitly requested"));
    assert_eq!(request["messages"][1]["content"], "Synthetic test result");
    server.release.send(()).unwrap();
    let snapshot = h.wait(|s| s.session.phase == Phase::Idle).await;
    let record = &snapshot.recent_results[0];
    assert!(
        record.needs_review,
        "AI editing must not clear ASR quality warnings"
    );
    assert_eq!(record.polish_outcome, "applied");
    assert_eq!(record.polish_reason, None);
    assert_eq!(record.raw_text, "Synthetic test result");
    assert_eq!(
        record.polished_text.as_deref(),
        Some("Synthetic test result.\n—clean")
    );
    assert_eq!(
        h.handle.result_text(id, "polished".into()).await.unwrap(),
        "Synthetic test result.\n—clean"
    );
    server.thread.join().unwrap();
    h.shutdown().await;
}
async fn patch(h: &Harness, patch: SettingsPatch) {
    h.handle
        .database(move |store| {
            store.update_settings(store.settings()?.revision, patch, crate::now())
        })
        .await
        .unwrap();
}
async fn raw_is_saved_once(h: &Harness, id: Uuid) {
    let snapshot = h.wait(|s| s.usage_today == 1).await;
    assert_eq!(snapshot.recent_results[0].raw_text, "Synthetic test result");
    assert!(snapshot.recent_results[0].polished_text.is_none());
    let raw = h
        .handle
        .database(move |s| s.find_result(id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.polish_outcome, "off");
    assert!(raw.polished_text.is_none());
}

#[tokio::test]
async fn actor_polishes_once_after_raw_is_cached_and_counted_with_frozen_mode() {
    let mut server = Server::new("Synthetic test result.");
    let (h, service) = setup(&server).await;
    let id = h.record().await;
    assert_eq!(
        service
            .enable_polishing(h.handle.snapshot().unwrap().settings.revision)
            .await
            .unwrap_err()
            .code,
        "Busy"
    );
    patch(
        &h,
        SettingsPatch {
            polish_mode: Some("email".into()),
            ..Default::default()
        },
    )
    .await;
    h.handle.stop(id, false).await.unwrap();
    let request = server.request().await;
    assert!(request["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("Keep my voice and meaning"));
    raw_is_saved_once(&h, id).await;
    server.release.send(()).unwrap();
    let final_state = h.wait(|s| s.session.phase == Phase::Idle).await;
    assert_eq!(final_state.usage_today, 1);
    let result = &final_state.recent_results[0];
    assert_eq!(result.polish_outcome, "applied");
    assert_eq!(
        result.polished_text.as_deref(),
        Some("Synthetic test result.")
    );
    assert_eq!(result.polish_reason, None);
    assert_eq!(result.delivery_status, "ready_for_copy");
    assert_eq!(
        h.handle.result_text(id, "raw".into()).await.unwrap(),
        "Synthetic test result"
    );
    assert_eq!(
        h.handle.result_text(id, "polished".into()).await.unwrap(),
        "Synthetic test result."
    );
    let saved = h
        .handle
        .database(move |s| s.find_result(id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.polished_text, result.polished_text);
    assert_eq!(saved.polish_ms, result.polish_ms);
    server.thread.join().unwrap();
    h.shutdown().await;
}

#[tokio::test]
async fn actor_polish_cancellation_privacy_deletion_deadline_and_rejection_keep_one_original() {
    for scenario in [
        "cancel",
        "native_cancel",
        "interrupt",
        "shutdown",
        "disable",
        "delete",
        "history_revoke",
        "deadline",
        "rejected",
        "clear_cache",
    ] {
        let mut server = Server::new(if scenario == "rejected" {
            "Synthetic test result 123."
        } else {
            "Synthetic test result."
        });
        let (h, _) = setup(&server).await;
        let id = h.record().await;
        h.handle.stop(id, false).await.unwrap();
        server.request().await;
        raw_is_saved_once(&h, id).await;
        match scenario {
            "cancel" => {
                h.handle.stop(id, true).await.unwrap();
            }
            "native_cancel" => {
                h.handle.native_stop(id, true).unwrap();
            }
            "interrupt" => {
                h.handle.native_input().interrupt(
                    super::super::super::native_input::Interruption::InactiveSession,
                    true,
                );
                h.handle.native_input().interrupt(
                    super::super::super::native_input::Interruption::InactiveSession,
                    false,
                );
            }
            "shutdown" => {
                h.handle.shutdown();
            }
            "disable" => {
                patch(
                    &h,
                    SettingsPatch {
                        polish_enabled: Some(false),
                        ..Default::default()
                    },
                )
                .await;
                patch(
                    &h,
                    SettingsPatch {
                        polish_enabled: Some(true),
                        ..Default::default()
                    },
                )
                .await;
            }
            "delete" => {
                h.handle
                    .database(move |s| s.delete_history(&[id]))
                    .await
                    .unwrap();
            }
            "history_revoke" => {
                patch(
                    &h,
                    SettingsPatch {
                        history_enabled: Some(false),
                        ..Default::default()
                    },
                )
                .await;
                patch(
                    &h,
                    SettingsPatch {
                        history_enabled: Some(true),
                        ..Default::default()
                    },
                )
                .await;
            }
            "deadline" => {
                h.wait(|s| s.session.phase == Phase::Idle).await;
            }
            "clear_cache" => {
                h.handle.clear_results().await.unwrap();
            }
            _ => (),
        }
        server.release.send(()).unwrap();
        let final_state = h.wait(|s| s.session.phase == Phase::Idle).await;
        assert_eq!(final_state.usage_today, 1, "{scenario}");
        let saved = h
            .store
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .find_result(id)
            .unwrap();
        if scenario == "delete" {
            assert!(saved.is_none());
        }
        if scenario == "history_revoke" {
            assert!(saved.as_ref().is_none_or(|r| r.polished_text.is_none()));
        }
        if scenario == "clear_cache" {
            assert!(final_state.recent_results.is_empty());
        } else {
            let result = &final_state.recent_results[0];
            assert_eq!(result.raw_text, "Synthetic test result");
            let reason = match scenario {
                "cancel" | "native_cancel" | "interrupt" | "shutdown" => Some(Reason::Cancelled),
                "disable" => Some(Reason::PolicyRevoked),
                "deadline" => Some(Reason::Deadline),
                "rejected" => Some(Reason::ResponseRejected),
                _ => None,
            };
            assert_eq!(result.polish_reason, reason, "{scenario}");
            assert_eq!(
                result.polished_text.is_none(),
                reason.is_some(),
                "{scenario}"
            );
            if reason.is_some() {
                if scenario != "shutdown" {
                    assert_eq!(
                        h.handle.result_text(id, "polished".into()).await.unwrap(),
                        "Synthetic test result"
                    );
                }
                assert_eq!(saved.unwrap().polish_reason, reason);
            }
        }
        server.thread.join().unwrap();
        h.shutdown().await;
    }
}

#[tokio::test]
async fn contextual_app_rule_is_frozen_and_sent_without_changing_the_raw_transcript() {
    let mut server = Server::new("Synthetic test result.");
    let (h, _service) = setup(&server).await;
    let rule = speakmore_core::writing::Rule {
        id: Uuid::new_v4(),
        kind: "app".into(),
        target: "com.speakmore.desktop".into(),
        display_name: "SpeakMore fixture".into(),
        prompt: "Use concise fixture paragraphs".into(),
        enabled: true,
    };
    let original = rule.clone();
    h.handle
        .database(move |store| {
            store.upsert_writing_rule(store.settings()?.revision, original, crate::now())
        })
        .await
        .unwrap();
    let id = h.record().await;
    let mut changed = rule;
    changed.prompt = "This changed preference must only apply next time".into();
    h.handle
        .database(move |store| {
            store.upsert_writing_rule(store.settings()?.revision, changed, crate::now())
        })
        .await
        .unwrap();
    h.handle.stop(id, false).await.unwrap();
    let request = server.request().await;
    let instructions = request["messages"][0]["content"].as_str().unwrap();
    assert!(instructions.contains("Use concise fixture paragraphs"));
    assert!(!instructions.contains("This changed preference"));
    assert!(instructions.contains("com.speakmore.desktop"));
    assert_eq!(request["messages"][1]["content"], "Synthetic test result");
    server.release.send(()).unwrap();
    let snapshot = h.wait(|s| s.session.phase == Phase::Idle).await;
    assert_eq!(snapshot.recent_results[0].raw_text, "Synthetic test result");
    assert_eq!(snapshot.writing_rule.as_deref(), Some("SpeakMore fixture"));
}
