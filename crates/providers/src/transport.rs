use crate::{
    guard, protocol, Failure, Mode, PreparedProvider, ProcessingPolicy, Result, DEADLINE,
    MAX_BODY_BYTES,
};
use reqwest::{header, redirect, retry, Client as HttpClient};
use std::{fmt, sync::Arc};
use tokio::{sync::Semaphore, time::Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const PROBE: &str = "Please add punctuation to this sentence";
/// Response shape: dictation edits and captions are short; long-form JSON is not.
#[derive(Clone, Copy)]
struct Output {
    structured: bool,
    tokens: u32,
}
const EDIT: Output = Output {
    structured: false,
    tokens: 2048,
};
const SHORT_JSON: Output = Output {
    structured: true,
    tokens: 2048,
};
const LONG_JSON: Output = Output {
    structured: true,
    tokens: 8192,
};

/// Set by the native controller from the saved refinement preference.
/// Cloud consent and a tested provider are checked independently.
pub enum NativeEligibility {
    Disabled,
    Enabled,
}

/// In-memory proof for this exact immutable provider and credential. A native
/// storage checkpoint can restore it after configuration/credential-revision
/// validation; it is never accepted as a WebView permission flag.
pub struct TestReceipt {
    pub(crate) provider: Uuid,
}

pub enum Outcome {
    Polished(String),
    /// The caller keeps its untouched raw text; no partial output is returned.
    Original(Failure),
}

impl fmt::Debug for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Polished(_) => f.write_str("Polished([REDACTED])"),
            Self::Original(reason) => f.debug_tuple("Original").field(reason).finish(),
        }
    }
}

pub struct PolishInput<'a> {
    pub provider: &'a PreparedProvider,
    pub receipt: &'a TestReceipt,
    pub raw_text: &'a str,
    pub mode: Mode<'a>,
    pub policy: ProcessingPolicy<'a>,
    pub eligibility: NativeEligibility,
}

/// A shared one-request slot covers both explicit connection tests and polishing.
/// Independent workloads (meeting summaries, captions) must use their own `Client`.
/// The provider attempt is not detached from its caller future. The HTTP library
/// may finish internal DNS/connection cleanup after cancellation, but no late
/// response can publish text or start a replacement provider attempt.
#[derive(Clone)]
pub struct Client {
    http: HttpClient,
    slot: Arc<Semaphore>,
    deadline: std::time::Duration,
    test_deadline: std::time::Duration,
}

impl Client {
    pub fn new() -> Result<Self> {
        let http = HttpClient::builder()
            .redirect(redirect::Policy::none())
            .retry(retry::never())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .referer(false)
            .pool_max_idle_per_host(0)
            .connect_timeout(DEADLINE)
            .timeout(DEADLINE)
            .build()
            .map_err(|_| Failure::NetworkUnavailable)?;
        Ok(Self {
            http,
            slot: Arc::new(Semaphore::new(1)),
            deadline: DEADLINE,
            test_deadline: std::time::Duration::from_secs(15),
        })
    }

    pub async fn test_connection(
        &self,
        provider: &PreparedProvider,
        policy: ProcessingPolicy<'_>,
        cancel: &CancellationToken,
    ) -> Result<TestReceipt> {
        let deadline = Instant::now() + self.test_deadline;
        let _ = self
            .execute(provider, PROBE, &Mode::Standard, policy, cancel, deadline)
            .await?;
        Ok(TestReceipt {
            provider: provider.identity,
        })
    }

    pub async fn polish(&self, input: PolishInput<'_>, cancel: &CancellationToken) -> Outcome {
        let deadline = Instant::now() + self.deadline;
        let failure = match input.eligibility {
            NativeEligibility::Disabled => Some(Failure::Disabled),
            NativeEligibility::Enabled if input.receipt.provider != input.provider.identity => {
                Some(Failure::ProfileNotTested)
            }
            NativeEligibility::Enabled => None,
        };
        if let Some(reason) = failure {
            return Outcome::Original(reason);
        }
        match self
            .execute(
                input.provider,
                input.raw_text,
                &input.mode,
                input.policy,
                cancel,
                deadline,
            )
            .await
        {
            Ok(text) => Outcome::Polished(text),
            Err(reason) => Outcome::Original(reason),
        }
    }

    async fn execute(
        &self,
        provider: &PreparedProvider,
        raw: &str,
        mode: &Mode<'_>,
        policy: ProcessingPolicy<'_>,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<String> {
        check_live(cancel, deadline)?;
        policy.check(&provider.config)?;
        guard::check_input(raw)?;
        let instructions = mode.instructions()?;
        self.request_text(provider, raw, &instructions, cancel, deadline, EDIT)
            .await
    }

    /// Explicit long-form JSON generation, separate from dictation's editing guards and budget.
    /// The native caller must validate the resulting JSON against its task schema and evidence.
    pub async fn generate_json(
        &self,
        provider: &PreparedProvider,
        receipt: &TestReceipt,
        raw: &str,
        instructions: &str,
        policy: ProcessingPolicy<'_>,
        cancel: &CancellationToken,
    ) -> Result<String> {
        if receipt.provider != provider.identity {
            return Err(Failure::ProfileNotTested);
        }
        policy.check(&provider.config)?;
        if raw.trim().is_empty() || raw.len() > 64 * 1024 {
            return Err(Failure::TextTooLong);
        }
        if instructions.trim().is_empty() || instructions.len() > 16 * 1024 {
            return Err(Failure::InvalidPrompt);
        }
        self.request_text(
            provider,
            raw,
            instructions,
            cancel,
            Instant::now() + std::time::Duration::from_secs(120),
            LONG_JSON,
        )
        .await
    }

    /// Short translation budget prevents a live-caption queue from growing behind a slow service.
    pub async fn translate(
        &self,
        provider: &PreparedProvider,
        receipt: &TestReceipt,
        raw: &str,
        instructions: &str,
        policy: ProcessingPolicy<'_>,
        cancel: &CancellationToken,
    ) -> Result<String> {
        if receipt.provider != provider.identity {
            return Err(Failure::ProfileNotTested);
        }
        policy.check(&provider.config)?;
        if raw.trim().is_empty() || raw.len() > 16 * 1024 {
            return Err(Failure::TextTooLong);
        }
        if instructions.trim().is_empty() || instructions.len() > 4096 {
            return Err(Failure::InvalidPrompt);
        }
        self.request_text(
            provider,
            raw,
            instructions,
            cancel,
            Instant::now() + std::time::Duration::from_secs(8),
            SHORT_JSON,
        )
        .await
    }

    async fn request_text(
        &self,
        provider: &PreparedProvider,
        raw: &str,
        instructions: &str,
        cancel: &CancellationToken,
        deadline: Instant,
        output: Output,
    ) -> Result<String> {
        check_live(cancel, deadline)?;
        let _permit = self.slot.try_acquire().map_err(|_| Failure::Busy)?;
        match self
            .attempt(provider, raw, instructions, cancel, deadline, output)
            .await
        {
            // Some models/servers cap output below the long-form budget and reject it with 400.
            // Retry once, under the same permit and deadline, with the budget the connection
            // test already proved; capable models keep the larger budget.
            Err(Failure::HttpStatus { status: 400 }) if output.tokens > SHORT_JSON.tokens => {
                let fallback = Output {
                    tokens: SHORT_JSON.tokens,
                    ..output
                };
                self.attempt(provider, raw, instructions, cancel, deadline, fallback)
                    .await
            }
            result => result,
        }
    }

    /// One HTTP request; the caller holds the shared slot permit.
    async fn attempt(
        &self,
        provider: &PreparedProvider,
        raw: &str,
        instructions: &str,
        cancel: &CancellationToken,
        deadline: Instant,
        output: Output,
    ) -> Result<String> {
        check_live(cancel, deadline)?;
        let (mut headers, body) = protocol::request(provider, raw, instructions, output.tokens)?;
        headers.insert(
            header::ACCEPT_ENCODING,
            header::HeaderValue::from_static("identity"),
        );
        check_live(cancel, deadline)?;
        // Keep the timeout outside send + complete bounded body read + parse/check.
        // No timeout reset per chunk; the only fallback is the long-form budget retry above.
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(Failure::Cancelled),
            result = tokio::time::timeout_at(deadline, async {
                let mut response = self.http.post(provider.config.endpoint.clone())
                    .headers(headers).body(body).timeout(deadline.saturating_duration_since(Instant::now())).send().await.map_err(network_error)?;
                if response.status().is_redirection() {
                    return Err(Failure::RedirectDisallowed);
                }
                if response.status() != reqwest::StatusCode::OK {
                    return Err(Failure::HttpStatus { status: response.status().as_u16() });
                }
                let content_type = response.headers().get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split(';').next())
                    .map(str::trim);
                if !content_type.is_some_and(|value| value.eq_ignore_ascii_case("application/json"))
                    || response.headers().get(header::CONTENT_ENCODING).is_some_and(|value| value != "identity")
                {
                    return Err(Failure::UnsupportedResponse);
                }
                if response.content_length().is_some_and(|n| n > MAX_BODY_BYTES as u64) {
                    return Err(Failure::BodyTooLarge);
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(network_error)? {
                    check_live(cancel, deadline)?;
                    if chunk.len() > MAX_BODY_BYTES - bytes.len() {
                        return Err(Failure::BodyTooLarge);
                    }
                    bytes.extend_from_slice(&chunk);
                }
                check_live(cancel, deadline)?;
                let text = protocol::response(provider.config.protocol, &bytes)?;
                if output.structured {
                    if text.trim().is_empty() || text.len() > 64 * 1024 { return Err(Failure::InvalidOutput); }
                } else { guard::check_output(raw, &text)?; }
                check_live(cancel, deadline)?;
                Ok(text)
            }) => result.unwrap_or(Err(Failure::Deadline)),
        };
        // Also discard a completion that races policy cancellation or deadline.
        check_live(cancel, deadline)?;
        result
    }
}

fn check_live(cancel: &CancellationToken, deadline: Instant) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Failure::Cancelled)
    } else if Instant::now() >= deadline {
        Err(Failure::Deadline)
    } else {
        Ok(())
    }
}

fn network_error(error: reqwest::Error) -> Failure {
    // reqwest errors can contain URLs: discard them instead of formatting/logging.
    if error.is_timeout() {
        Failure::Deadline
    } else {
        Failure::NetworkUnavailable
    }
}

#[cfg(test)]
impl Client {
    pub(crate) fn with_deadline(deadline: std::time::Duration) -> Self {
        Self {
            deadline,
            test_deadline: deadline,
            ..Self::new().expect("test client")
        }
    }
}

#[cfg(test)]
impl TestReceipt {
    pub(crate) fn fixture(provider: &PreparedProvider) -> Self {
        Self {
            provider: provider.identity,
        }
    }
}
