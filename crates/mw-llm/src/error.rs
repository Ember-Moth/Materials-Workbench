use std::fmt;
use std::time::Duration;

/// Why a model call failed.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum LlmError {
    /// The request could not be built from the configuration or input.
    InvalidRequest(String),
    /// The service could not be reached, or the connection broke.
    Transport(String),
    /// The service reported an error: an HTTP error status, or an error event
    /// in the middle of a stream (`status` is then `None`).
    Api(ApiError),
    /// The response did not have the expected shape.
    Decode(String),
    /// The stream ended before the response was complete.
    Incomplete,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApiError {
    pub status: Option<u16>,
    /// The vendor's error type or code, such as `overloaded_error` or
    /// `rate_limit_exceeded`.
    pub kind: Option<String>,
    pub message: String,
    /// Quote this when reporting a problem to the vendor.
    pub request_id: Option<String>,
    /// From the `retry-after` header, when the service sent one.
    pub retry_after: Option<Duration>,
}

/// Error kinds that mean "try again later", from both vendors' docs.
const RETRYABLE_KINDS: &[&str] = &[
    // Anthropic
    "overloaded_error",
    "api_error",
    "rate_limit_error",
    "timeout_error",
    // OpenAI
    "server_error",
    "rate_limit_exceeded",
];

impl LlmError {
    /// Whether the same request may succeed if sent again (design §6.2.3):
    /// 429 except an exhausted quota, 5xx including Anthropic's 529, the same
    /// conditions reported as stream error events, and connection failures.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport(_) => true,
            Self::Api(error) => {
                if error.kind.as_deref() == Some("insufficient_quota") {
                    return false;
                }
                match error.status {
                    Some(status) => status == 429 || (500..600).contains(&status),
                    None => error
                        .kind
                        .as_deref()
                        .is_some_and(|kind| RETRYABLE_KINDS.contains(&kind)),
                }
            }
            Self::InvalidRequest(_) | Self::Decode(_) | Self::Incomplete => false,
        }
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "invalid request: {message}"),
            Self::Transport(message) => write!(f, "connection failed: {message}"),
            Self::Api(error) => {
                match error.status {
                    Some(status) => write!(f, "HTTP {status}")?,
                    None => write!(f, "stream error")?,
                }
                if let Some(kind) = &error.kind {
                    write!(f, " ({kind})")?;
                }
                write!(f, ": {}", error.message)?;
                if let Some(id) = &error.request_id {
                    write!(f, " [request {id}]")?;
                }
                Ok(())
            }
            Self::Decode(message) => write!(f, "unexpected response: {message}"),
            Self::Incomplete => f.write_str("the response ended before it was complete"),
        }
    }
}

impl std::error::Error for LlmError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(status: u16, kind: Option<&str>) -> LlmError {
        LlmError::Api(ApiError {
            status: Some(status),
            kind: kind.map(str::to_owned),
            ..ApiError::default()
        })
    }

    fn event(kind: &str) -> LlmError {
        LlmError::Api(ApiError {
            kind: Some(kind.to_owned()),
            ..ApiError::default()
        })
    }

    #[test]
    fn retries_rate_limits_and_server_errors_only() {
        assert!(http(429, Some("rate_limit_error")).is_retryable());
        assert!(http(529, Some("overloaded_error")).is_retryable());
        assert!(http(500, None).is_retryable());
        assert!(http(503, None).is_retryable());
        assert!(!http(400, Some("invalid_request_error")).is_retryable());
        assert!(!http(401, None).is_retryable());
        assert!(!http(429, Some("insufficient_quota")).is_retryable());
        assert!(LlmError::Transport("reset".into()).is_retryable());
        assert!(!LlmError::Decode("bad".into()).is_retryable());
    }

    #[test]
    fn retries_stream_errors_by_kind() {
        assert!(event("overloaded_error").is_retryable());
        assert!(event("server_error").is_retryable());
        assert!(!event("invalid_request_error").is_retryable());
        assert!(!event("something_new").is_retryable());
    }
}
