use serde::Serialize;

/// Deliberately contains no URL, HTTP body, prompt, credential or upstream error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Failure {
    #[error("invalid_endpoint")]
    InvalidEndpoint,
    #[error("invalid_model")]
    InvalidModel,
    #[error("invalid_credential")]
    InvalidCredential,
    #[error("credential_required")]
    CredentialRequired,
    #[error("credential_not_supported")]
    CredentialNotSupported,
    #[error("local_only")]
    LocalOnly,
    #[error("destination_consent_required")]
    DestinationConsentRequired,
    #[error("profile_not_tested")]
    ProfileNotTested,
    #[error("disabled")]
    Disabled,
    #[error("busy")]
    Busy,
    #[error("cancelled")]
    Cancelled,
    #[error("deadline")]
    Deadline,
    #[error("text_too_long")]
    TextTooLong,
    #[error("invalid_text")]
    InvalidText,
    #[error("invalid_prompt")]
    InvalidPrompt,
    #[error("network_unavailable")]
    NetworkUnavailable,
    #[error("redirect_disallowed")]
    RedirectDisallowed,
    #[error("http_status_{status}")]
    HttpStatus { status: u16 },
    #[error("body_too_large")]
    BodyTooLarge,
    #[error("unsupported_response")]
    UnsupportedResponse,
    #[error("incomplete_response")]
    IncompleteResponse,
    #[error("refused_response")]
    RefusedResponse,
    #[error("empty_response")]
    EmptyResponse,
    #[error("invalid_output")]
    InvalidOutput,
    #[error("protected_content_changed")]
    ProtectedContentChanged,
}

pub type Result<T> = std::result::Result<T, Failure>;
