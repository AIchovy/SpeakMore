use super::*;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
enum Reply {
    Echo(Protocol),
    Json(Value),
    Status(u16, Option<String>),
    AdvertisedOversize,
    ChunkedOversize,
    SlowBody,
    Stall,
    Disconnect,
    BadEncoding,
    /// 400 when the requested output budget exceeds the model's cap, else this body.
    BudgetCap(u64, Value),
}

struct Captured {
    headers: String,
    body: Value,
}

struct Server {
    endpoint: String,
    requests: Arc<Mutex<Vec<Captured>>>,
    received: Arc<Notify>,
    task: JoinHandle<()>,
}

impl Server {
    async fn new(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/api/chat", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = Arc::new(Notify::new());
        let (records, notify) = (requests.clone(), received.clone());
        let task = tokio::spawn(async move {
            let mut jobs = JoinSet::new();
            loop {
                tokio::select! {
                    socket = listener.accept() => {
                        let Ok((socket, _)) = socket else { break; };
                        let (records, notify, reply) = (records.clone(), notify.clone(), reply.clone());
                        jobs.spawn(async move {
                            let _ = serve(socket, records, notify, reply).await;
                        });
                    }
                    Some(result) = jobs.join_next(), if !jobs.is_empty() => { result.unwrap(); }
                }
            }
        });
        Self {
            endpoint,
            requests,
            received,
            task,
        }
    }

    fn provider(&self, protocol: Protocol) -> PreparedProvider {
        if protocol == Protocol::Ollama {
            PreparedProvider::new(
                Configuration::validate(input(protocol, &self.endpoint)).unwrap(),
                None,
            )
            .unwrap()
        } else {
            let mut provider = configured(protocol);
            // Unit-test module only: a wire fixture for common transport, not a
            // release feature or a public insecure-HTTP endpoint override.
            provider.config.endpoint = url::Url::parse(&self.endpoint).unwrap();
            provider
        }
    }

    async fn count(&self) -> usize {
        self.requests.lock().await.len()
    }

    async fn wait_for(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.count().await < count {
                self.received.notified().await;
            }
        })
        .await
        .expect("fixture request received");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    mut socket: TcpStream,
    records: Arc<Mutex<Vec<Captured>>>,
    notify: Arc<Notify>,
    reply: Reply,
) -> std::io::Result<()> {
    let mut bytes = Vec::new();
    let (head_end, body_length) = loop {
        let mut chunk = [0; 4096];
        let n = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut chunk)).await??;
        if n == 0 {
            return Ok(());
        }
        bytes.extend_from_slice(&chunk[..n]);
        assert!(bytes.len() <= 64 * 1024, "bounded test request");
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if bytes.len() >= end + 4 + length {
                break (end + 4, length);
            }
        }
    };
    let headers = String::from_utf8(bytes[..head_end].to_vec())
        .unwrap()
        .to_ascii_lowercase();
    let body: Value = serde_json::from_slice(&bytes[head_end..head_end + body_length]).unwrap();
    let raw = body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_owned();
    let over_cap = matches!(reply, Reply::BudgetCap(cap, _) if budget(&body) > cap);
    records.lock().await.push(Captured { headers, body });
    notify.notify_one();
    match reply {
        Reply::Echo(protocol) => {
            let text = format!("{}.", raw.trim_end_matches('.'));
            let body = match protocol {
                Protocol::Anthropic => anthropic(&text),
                Protocol::Ollama => ollama(&text),
                _ => chat(&text),
            };
            send_json(&mut socket, body).await?;
        }
        Reply::Json(body) => send_json(&mut socket, body).await?,
        Reply::BudgetCap(_, body) => {
            if over_cap {
                status(&mut socket, 400, String::new()).await?;
            } else {
                send_json(&mut socket, body).await?;
            }
        }
        Reply::Status(code, location) => {
            let location = location
                .map(|url| format!("Location: {url}\r\n"))
                .unwrap_or_default();
            status(&mut socket, code, location).await?;
        }
        Reply::AdvertisedOversize => {
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", MAX_BODY_BYTES + 1).as_bytes()).await?;
            std::future::pending::<()>().await;
        }
        Reply::ChunkedOversize => {
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n").await?;
            let chunk = vec![b' '; 32 * 1024];
            for _ in 0..9 {
                socket
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .await?;
                socket.write_all(&chunk).await?;
                socket.write_all(b"\r\n").await?;
            }
            socket.write_all(b"0\r\n\r\n").await?;
        }
        Reply::SlowBody => {
            let body = serde_json::to_vec(&ollama("Waiting.")).unwrap();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes()).await?;
            for chunk in body.chunks(10) {
                socket.write_all(chunk).await?;
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
        }
        Reply::Stall => std::future::pending::<()>().await,
        Reply::Disconnect => {}
        Reply::BadEncoding => {
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nContent-Length: 2\r\n\r\n{}").await?;
        }
    }
    socket.shutdown().await
}

async fn status(socket: &mut TcpStream, status: u16, location: String) -> std::io::Result<()> {
    let body = b"private-upstream-body-secret-do-not-echo";
    socket.write_all(format!("HTTP/1.1 {status} Fixture\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await?;
    socket.write_all(body).await
}

fn budget(body: &Value) -> u64 {
    ["max_tokens", "max_completion_tokens"]
        .iter()
        .find_map(|field| body[field].as_u64())
        .or_else(|| body["options"]["num_predict"].as_u64())
        .unwrap()
}

async fn send_json(socket: &mut TcpStream, body: Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(&body).unwrap();
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).as_bytes()).await?;
    socket.write_all(&bytes).await
}

#[tokio::test]
async fn explicit_probe_then_polish_round_trip_all_protocols() {
    for protocol in [
        Protocol::Openai,
        Protocol::Anthropic,
        Protocol::Ollama,
        Protocol::OpenaiCompatible {
            budget_field: BudgetField::MaxTokens,
            instruction_role: InstructionRole::Developer,
        },
    ] {
        let server = Server::new(Reply::Echo(protocol)).await;
        let provider = server.provider(protocol);
        let consent = provider.config.confirm_destination();
        let client = Client::new().unwrap();
        let cancel = CancellationToken::new();
        let receipt = client
            .test_connection(&provider, ProcessingPolicy::AllowCloud(&consent), &cancel)
            .await
            .unwrap();
        let outcome = client
            .polish(
                PolishInput {
                    provider: &provider,
                    receipt: &receipt,
                    raw_text: "Meet at 12",
                    mode: Mode::Email,
                    policy: ProcessingPolicy::AllowCloud(&consent),
                    eligibility: NativeEligibility::Enabled,
                },
                &cancel,
            )
            .await;
        assert!(
            matches!(outcome, Outcome::Polished(ref text) if text == "Meet at 12."),
            "{outcome:?}"
        );
        let requests = server.requests.lock().await;
        assert_eq!(requests.len(), 2);
        let probe = requests[0].body["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["content"]
            .as_str()
            .unwrap();
        assert_eq!(probe, "Please add punctuation to this sentence");
        assert!(!serde_json::to_string(&requests[0].body)
            .unwrap()
            .contains("Meet at"));
        assert!(requests[1].headers.starts_with("post /api/chat http/1.1"));
        assert!(requests[1].headers.contains("accept-encoding: identity"));
        if protocol == Protocol::Openai {
            assert!(requests[1]
                .headers
                .contains("authorization: bearer fixture-secret-never-real"));
        }
        if protocol == Protocol::Anthropic {
            assert!(requests[1]
                .headers
                .contains("x-api-key: fixture-secret-never-real"));
        }
    }
}

#[tokio::test]
async fn policy_eligibility_cancel_and_input_limits_send_nothing() {
    let server = Server::new(Reply::Echo(Protocol::Openai)).await;
    let provider = server.provider(Protocol::Openai);
    let receipt = TestReceipt::fixture(&provider);
    let client = Client::new().unwrap();
    let consent = provider.config.confirm_destination();
    let cancel = CancellationToken::new();
    assert!(matches!(
        client
            .test_connection(&provider, ProcessingPolicy::LocalOnly, &cancel)
            .await,
        Err(Failure::LocalOnly)
    ));
    for (eligibility, expected) in [(NativeEligibility::Disabled, Failure::Disabled)] {
        assert!(matches!(client.polish(PolishInput {
            provider: &provider, receipt: &receipt, raw_text: "Private words", mode: Mode::Standard,
            policy: ProcessingPolicy::AllowCloud(&consent), eligibility,
        }, &cancel).await, Outcome::Original(reason) if reason == expected));
    }
    let oversized = "中".repeat(4001);
    assert!(matches!(
        client
            .polish(
                PolishInput {
                    provider: &provider,
                    receipt: &receipt,
                    raw_text: &oversized,
                    mode: Mode::Standard,
                    policy: ProcessingPolicy::AllowCloud(&consent),
                    eligibility: NativeEligibility::Enabled,
                },
                &cancel
            )
            .await,
        Outcome::Original(Failure::TextTooLong)
    ));
    let other = configured(Protocol::Openai);
    let wrong_receipt = TestReceipt::fixture(&other);
    assert!(matches!(
        client
            .polish(
                PolishInput {
                    provider: &provider,
                    receipt: &wrong_receipt,
                    raw_text: "Private",
                    mode: Mode::Standard,
                    policy: ProcessingPolicy::AllowCloud(&consent),
                    eligibility: NativeEligibility::Enabled,
                },
                &cancel
            )
            .await,
        Outcome::Original(Failure::ProfileNotTested)
    ));
    let wrong_consent = other.config.confirm_destination();
    assert!(matches!(
        client
            .test_connection(
                &provider,
                ProcessingPolicy::AllowCloud(&wrong_consent),
                &cancel
            )
            .await,
        Err(Failure::DestinationConsentRequired)
    ));
    cancel.cancel();
    assert!(matches!(
        client
            .test_connection(&provider, ProcessingPolicy::AllowCloud(&consent), &cancel)
            .await,
        Err(Failure::Cancelled)
    ));
    assert_eq!(server.count().await, 0);
}

#[tokio::test]
async fn key_rotation_invalidates_a_successful_test_receipt() {
    let server = Server::new(Reply::Echo(Protocol::Openai)).await;
    let old = server.provider(Protocol::Openai);
    let consent = old.config.confirm_destination();
    let client = Client::new().unwrap();
    let cancel = CancellationToken::new();
    let receipt = client
        .test_connection(&old, ProcessingPolicy::AllowCloud(&consent), &cancel)
        .await
        .unwrap();
    let rotated = PreparedProvider::new(
        old.config.clone(),
        Some(SecretKey::new("new-fixture".into()).unwrap()),
    )
    .unwrap();
    let outcome = client
        .polish(
            PolishInput {
                provider: &rotated,
                receipt: &receipt,
                raw_text: "Private",
                mode: Mode::Standard,
                policy: ProcessingPolicy::AllowCloud(&consent),
                eligibility: NativeEligibility::Enabled,
            },
            &cancel,
        )
        .await;
    assert!(matches!(
        outcome,
        Outcome::Original(Failure::ProfileNotTested)
    ));
    assert_eq!(server.count().await, 1);
}

#[tokio::test]
async fn redirects_are_never_followed_and_failures_never_retry() {
    let target = Server::new(Reply::Echo(Protocol::Ollama)).await;
    let redirect = Server::new(Reply::Status(307, Some(target.endpoint.clone()))).await;
    let client = Client::new().unwrap();
    let provider = redirect.provider(Protocol::Ollama);
    let cancel = CancellationToken::new();
    assert!(matches!(
        client
            .test_connection(&provider, ProcessingPolicy::LocalOnly, &cancel)
            .await,
        Err(Failure::RedirectDisallowed)
    ));
    assert_eq!(redirect.count().await, 1);
    assert_eq!(target.count().await, 0);
    for status in [401, 403, 429, 500, 503] {
        let server = Server::new(Reply::Status(status, None)).await;
        let provider = server.provider(Protocol::Ollama);
        let reason = match client
            .test_connection(&provider, ProcessingPolicy::LocalOnly, &cancel)
            .await
        {
            Err(reason) => reason,
            Ok(_) => panic!("unexpected receipt"),
        };
        assert_eq!(reason, Failure::HttpStatus { status });
        let serialized = serde_json::to_string(&reason).unwrap();
        assert!(!serialized.contains("private-upstream"));
        assert!(!serialized.contains("http://"));
        assert_eq!(server.count().await, 1);
    }
    let broken = Server::new(Reply::Disconnect).await;
    let provider = broken.provider(Protocol::Ollama);
    assert!(matches!(
        client
            .test_connection(&provider, ProcessingPolicy::LocalOnly, &cancel)
            .await,
        Err(Failure::NetworkUnavailable)
    ));
    assert_eq!(broken.count().await, 1);
}

#[tokio::test]
async fn bounded_body_handles_missing_length_and_disallows_compression() {
    let client = Client::new().unwrap();
    for (reply, expected) in [
        (Reply::AdvertisedOversize, Failure::BodyTooLarge),
        (Reply::ChunkedOversize, Failure::BodyTooLarge),
        (Reply::BadEncoding, Failure::UnsupportedResponse),
    ] {
        let server = Server::new(reply).await;
        let provider = server.provider(Protocol::Ollama);
        let result = client
            .test_connection(
                &provider,
                ProcessingPolicy::LocalOnly,
                &CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Err(reason) if reason == expected));
        assert_eq!(server.count().await, 1);
    }
}

#[tokio::test]
async fn total_deadline_is_not_reset_by_arriving_body_chunks() {
    let server = Server::new(Reply::SlowBody).await;
    let provider = server.provider(Protocol::Ollama);
    let client = Client::with_deadline(Duration::from_millis(250));
    let started = tokio::time::Instant::now();
    let result = client
        .test_connection(
            &provider,
            ProcessingPolicy::LocalOnly,
            &CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(Failure::Deadline)));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(server.count().await, 1);
}

#[tokio::test]
async fn cancellation_busy_and_dropped_call_release_the_shared_slot() {
    let server = Server::new(Reply::Stall).await;
    let healthy = Server::new(Reply::Echo(Protocol::Ollama)).await;
    let client = Client::new().unwrap();
    let next = healthy.provider(Protocol::Ollama);
    for abort_caller in [false, true] {
        let provider = server.provider(Protocol::Ollama);
        let (worker, cancel) = (client.clone(), CancellationToken::new());
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            worker
                .test_connection(&provider, ProcessingPolicy::LocalOnly, &token)
                .await
        });
        server.wait_for(if abort_caller { 2 } else { 1 }).await;
        assert!(matches!(
            client
                .test_connection(
                    &next,
                    ProcessingPolicy::LocalOnly,
                    &CancellationToken::new()
                )
                .await,
            Err(Failure::Busy)
        ));
        if abort_caller {
            task.abort();
            assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        } else {
            cancel.cancel();
            assert!(matches!(task.await.unwrap(), Err(Failure::Cancelled)));
        }
        client
            .test_connection(
                &next,
                ProcessingPolicy::LocalOnly,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    assert_eq!(healthy.count().await, 2);
    assert_eq!(server.count().await, 2);
}

#[tokio::test]
async fn tls_failure_never_falls_back_to_plain_http_or_exposes_a_key() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut handshake = [0; 8192];
        let n = socket.read(&mut handshake).await.unwrap();
        assert!(n > 0);
        assert!(!String::from_utf8_lossy(&handshake[..n]).contains("fixture-secret"));
        assert!(!handshake[..n].starts_with(b"POST"));
        // A plaintext server cannot authenticate a TLS endpoint. No test-only
        // certificate-verification bypass is used for this failure check.
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        socket.shutdown().await.unwrap();
    });
    let configuration =
        Configuration::validate(input(Protocol::Openai, &format!("https://{address}/chat")))
            .unwrap();
    let consent = configuration.confirm_destination();
    let provider = PreparedProvider::new(
        configuration,
        Some(SecretKey::new("fixture-secret".into()).unwrap()),
    )
    .unwrap();
    let result = Client::new()
        .unwrap()
        .test_connection(
            &provider,
            ProcessingPolicy::AllowCloud(&consent),
            &CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(Failure::NetworkUnavailable)));
    server.await.unwrap();
}

#[tokio::test]
async fn unsafe_or_partial_results_return_only_the_original_reason() {
    let mut partial = ollama("new text");
    partial["done_reason"] = json!("length");
    for (body, expected) in [
        (ollama("Meet at 13"), Failure::ProtectedContentChanged),
        (ollama(" "), Failure::EmptyResponse),
        (ollama("Meet at 12\0"), Failure::InvalidOutput),
        (partial, Failure::IncompleteResponse),
    ] {
        let server = Server::new(Reply::Json(body)).await;
        let provider = server.provider(Protocol::Ollama);
        let receipt = TestReceipt::fixture(&provider);
        let raw = String::from("Meet at 12");
        let result = Client::new()
            .unwrap()
            .polish(
                PolishInput {
                    provider: &provider,
                    receipt: &receipt,
                    raw_text: &raw,
                    mode: Mode::Standard,
                    policy: ProcessingPolicy::LocalOnly,
                    eligibility: NativeEligibility::Enabled,
                },
                &CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Outcome::Original(reason) if reason == expected));
        assert_eq!(raw, "Meet at 12");
        assert_eq!(server.count().await, 1);
    }
}

#[tokio::test]
async fn meeting_json_uses_task_prompt_without_dictation_number_preservation_guard() {
    let response = json!({"message":{"role":"assistant","content":"{\"items\":[]}"},"done":true,"done_reason":"stop"});
    let server = Server::new(Reply::Json(response)).await;
    let provider = server.provider(Protocol::Ollama);
    let receipt = TestReceipt::fixture(&provider);
    let result = Client::new()
        .unwrap()
        .generate_json(
            &provider,
            &receipt,
            "Budget was 50000; no final decision.",
            "Return JSON meeting items only.",
            ProcessingPolicy::LocalOnly,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result, "{\"items\":[]}");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["messages"][0]["content"],
        "Return JSON meeting items only."
    );
    // Long-form output must not inherit the short dictation budget.
    assert_eq!(requests[0].body["options"]["num_predict"], 8192);
    assert!(!requests[0]
        .body
        .to_string()
        .contains("You edit dictated text"));
}

#[tokio::test]
async fn meeting_json_retries_once_with_short_budget_when_model_rejects_long_output() {
    for protocol in [
        Protocol::Openai,
        Protocol::Anthropic,
        Protocol::Ollama,
        Protocol::OpenaiCompatible {
            budget_field: BudgetField::MaxTokens,
            instruction_role: InstructionRole::System,
        },
    ] {
        let response = match protocol {
            Protocol::Anthropic => anthropic("{\"items\":[]}"),
            Protocol::Ollama => ollama("{\"items\":[]}"),
            _ => chat("{\"items\":[]}"),
        };
        // e.g. Claude 3 Haiku / gpt-4-turbo: max output 4096.
        let server = Server::new(Reply::BudgetCap(4096, response)).await;
        let provider = server.provider(protocol);
        let receipt = TestReceipt::fixture(&provider);
        let consent = provider.confirm_destination();
        let client = Client::new().unwrap();
        let result = client
            .generate_json(
                &provider,
                &receipt,
                "data",
                "Return JSON",
                ProcessingPolicy::AllowCloud(&consent),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result, "{\"items\":[]}");
        let budgets: Vec<_> = server
            .requests
            .lock()
            .await
            .iter()
            .map(|request| budget(&request.body))
            .collect();
        assert_eq!(budgets, [8192, 2048]);
        // The retry released the single shared slot.
        client
            .generate_json(
                &provider,
                &receipt,
                "data",
                "Return JSON",
                ProcessingPolicy::AllowCloud(&consent),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(server.count().await, 4);
    }
    // A model that rejects the short budget too fails after exactly one retry,
    // and short-budget requests never retry.
    let server = Server::new(Reply::BudgetCap(1024, ollama("{}"))).await;
    let provider = server.provider(Protocol::Ollama);
    let receipt = TestReceipt::fixture(&provider);
    let client = Client::new().unwrap();
    let cancel = CancellationToken::new();
    assert_eq!(
        client
            .generate_json(
                &provider,
                &receipt,
                "data",
                "Return JSON",
                ProcessingPolicy::LocalOnly,
                &cancel
            )
            .await
            .unwrap_err(),
        Failure::HttpStatus { status: 400 }
    );
    assert_eq!(server.count().await, 2);
    assert_eq!(
        client
            .translate(
                &provider,
                &receipt,
                "text",
                "translate",
                ProcessingPolicy::LocalOnly,
                &cancel
            )
            .await
            .unwrap_err(),
        Failure::HttpStatus { status: 400 }
    );
    assert_eq!(server.count().await, 3);
}

#[tokio::test]
async fn meeting_json_rejects_untested_or_disallowed_profiles_before_network() {
    let server = Server::new(Reply::Stall).await;
    let provider = server.provider(Protocol::Openai);
    let other = server.provider(Protocol::Openai);
    let client = Client::new().unwrap();
    assert_eq!(
        client
            .generate_json(
                &provider,
                &TestReceipt::fixture(&other),
                "data",
                "JSON",
                ProcessingPolicy::LocalOnly,
                &CancellationToken::new()
            )
            .await
            .unwrap_err(),
        Failure::ProfileNotTested
    );
    assert_eq!(
        client
            .generate_json(
                &provider,
                &TestReceipt::fixture(&provider),
                "data",
                "JSON",
                ProcessingPolicy::LocalOnly,
                &CancellationToken::new()
            )
            .await
            .unwrap_err(),
        Failure::LocalOnly
    );
    assert_eq!(server.count().await, 0);
}

#[tokio::test]
async fn meeting_json_cancellation_drops_pending_request_without_retry() {
    let server = Server::new(Reply::Stall).await;
    let provider = server.provider(Protocol::Ollama);
    let receipt = TestReceipt::fixture(&provider);
    let client = Client::new().unwrap();
    let cancel = CancellationToken::new();
    let run = client.generate_json(
        &provider,
        &receipt,
        "data",
        "Return JSON",
        ProcessingPolicy::LocalOnly,
        &cancel,
    );
    let (result, _) = tokio::join!(run, async {
        server.wait_for(1).await;
        cancel.cancel();
    });
    assert_eq!(result.unwrap_err(), Failure::Cancelled);
    assert_eq!(server.count().await, 1);
}

#[tokio::test]
async fn caption_translation_reuses_all_saved_protocols_and_task_instructions() {
    for protocol in [
        Protocol::Openai,
        Protocol::Anthropic,
        Protocol::Ollama,
        Protocol::OpenaiCompatible {
            budget_field: BudgetField::MaxTokens,
            instruction_role: InstructionRole::System,
        },
    ] {
        let translated = "Do not release before Friday.";
        let response = match protocol {
            Protocol::Anthropic => anthropic(translated),
            Protocol::Ollama => ollama(translated),
            _ => chat(translated),
        };
        let server = Server::new(Reply::Json(response)).await;
        let provider = server.provider(protocol);
        let receipt = TestReceipt::fixture(&provider);
        let consent = provider.confirm_destination();
        let output = Client::new()
            .unwrap()
            .translate(
                &provider,
                &receipt,
                "周五之前不要发布。",
                "Translate to English. Return only the translation.",
                ProcessingPolicy::AllowCloud(&consent),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(output, translated);
        let requests = server.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].body["model"], "fixture-model");
        assert!(!requests[0]
            .body
            .to_string()
            .contains("You edit dictated text"));
        assert!(!requests[0].body.to_string().contains("json_schema"));
    }
}

#[tokio::test]
async fn caption_translation_respects_policy_and_cancellation_without_fallback() {
    let server = Server::new(Reply::Stall).await;
    let provider = server.provider(Protocol::Openai);
    let receipt = TestReceipt::fixture(&provider);
    let client = Client::new().unwrap();
    let cancel = CancellationToken::new();
    assert_eq!(
        client
            .translate(
                &provider,
                &receipt,
                "text",
                "translate",
                ProcessingPolicy::LocalOnly,
                &cancel
            )
            .await
            .unwrap_err(),
        Failure::LocalOnly
    );
    assert_eq!(server.count().await, 0);
    let consent = provider.confirm_destination();
    let run = client.translate(
        &provider,
        &receipt,
        "text",
        "translate",
        ProcessingPolicy::AllowCloud(&consent),
        &cancel,
    );
    let (result, _) = tokio::join!(run, async {
        server.wait_for(1).await;
        cancel.cancel();
    });
    assert_eq!(result.unwrap_err(), Failure::Cancelled);
    assert_eq!(server.count().await, 1);
}
