//! The interface the assistant talks to (design §6.2.2), and how to connect a
//! configured provider.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::Stream;
use futures::stream::BoxStream;
use tokio::task::AbortHandle;

use crate::capabilities::{CapabilityOverrides, EndpointProfile};
use crate::driver::RetryPolicy;
use crate::{AssistantTurn, Backend, Capabilities, ChatRequest, LlmError, Origin, Runtime};

/// What a running request reports, in order. The last event of a successful
/// stream is [`StreamEvent::Done`]; a failed stream ends with an error item.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum StreamEvent {
    TextDelta(String),
    /// Readable reasoning summary text.
    ReasoningDelta(String),
    ToolCallStart {
        id: String,
        name: String,
    },
    /// The next piece of a call's arguments, a fragment of JSON text.
    ToolCallDelta {
        id: String,
        arguments: String,
    },
    /// The complete turn, assembled from everything above.
    Done(Box<AssistantTurn>),
}

/// The events of one request. It can be polled from any executor; dropping
/// it cancels the request.
pub struct ChatStream {
    events: BoxStream<'static, Result<StreamEvent, LlmError>>,
    task: Option<AbortHandle>,
}

impl ChatStream {
    /// A stream that replays fixed events, for scripted providers in tests.
    pub fn from_events(events: Vec<Result<StreamEvent, LlmError>>) -> Self {
        Self {
            events: Box::pin(futures::stream::iter(events)),
            task: None,
        }
    }

    pub(crate) fn failed(error: LlmError) -> Self {
        Self::from_events(vec![Err(error)])
    }

    pub(crate) fn new(
        events: BoxStream<'static, Result<StreamEvent, LlmError>>,
        task: AbortHandle,
    ) -> Self {
        Self {
            events,
            task: Some(task),
        }
    }
}

impl Stream for ChatStream {
    type Item = Result<StreamEvent, LlmError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.as_mut().poll_next(cx)
    }
}

impl Drop for ChatStream {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl fmt::Debug for ChatStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatStream").finish_non_exhaustive()
    }
}

/// A model service the assistant can call.
pub trait ChatProvider: Send + Sync {
    fn backend(&self) -> Backend;

    /// The base URL requests go to.
    fn endpoint(&self) -> &str;

    /// What `model` supports here, with the user's overrides applied.
    fn capabilities(&self, model: &str) -> Capabilities;

    /// Starts a request. Configuration and input errors arrive as the
    /// stream's first and only item.
    fn stream(&self, request: ChatRequest) -> ChatStream;

    /// The origin turns from `model` here are recorded with.
    fn origin(&self, model: &str) -> Origin {
        Origin {
            backend: self.backend(),
            endpoint: self.endpoint().to_owned(),
            model: model.to_owned(),
        }
    }
}

/// An API key. It is kept in memory only, never formatted or logged.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(…)")
    }
}

/// Connection timeouts. There is no limit on a whole request: long turns
/// stream for many minutes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    /// Longest silence between two chunks of the response.
    pub idle: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(30),
            idle: Duration::from_secs(600),
        }
    }
}

/// Everything needed to talk to one service (design §6.2.8).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ProviderConfig {
    pub profile: EndpointProfile,
    /// Defaults to the profile's.
    pub base_url: String,
    pub api_key: Option<ApiKey>,
    pub overrides: CapabilityOverrides,
    pub retry: RetryPolicy,
    pub timeouts: Timeouts,
}

impl ProviderConfig {
    pub fn new(profile: &EndpointProfile, api_key: Option<ApiKey>) -> Self {
        Self {
            profile: *profile,
            base_url: profile.base_url.to_owned(),
            api_key,
            overrides: CapabilityOverrides::default(),
            retry: RetryPolicy::default(),
            timeouts: Timeouts::default(),
        }
    }

    /// A user-added endpoint, with the most conservative capabilities.
    pub fn custom(backend: Backend, base_url: impl Into<String>, api_key: Option<ApiKey>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::new(&EndpointProfile::custom(backend), api_key)
        }
    }
}

/// Creates the provider for `config`. Requests run on `runtime`.
pub fn connect(
    config: ProviderConfig,
    runtime: &Runtime,
) -> Result<Arc<dyn ChatProvider>, LlmError> {
    use crate::{anthropic, driver::HttpProvider, openai_chat, openai_responses};

    let dialect = config.profile.dialect;
    Ok(match config.profile.backend {
        Backend::Anthropic => Arc::new(HttpProvider::new(
            anthropic::Wire::new(dialect),
            config,
            runtime,
        )?),
        Backend::OpenAiChat => Arc::new(HttpProvider::new(
            openai_chat::Wire::new(dialect),
            config,
            runtime,
        )?),
        Backend::OpenAiResponses => {
            Arc::new(HttpProvider::new(openai_responses::Wire, config, runtime)?)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[test]
    fn api_keys_never_print() {
        let key = ApiKey::new("sk-secret");
        assert_eq!(format!("{key:?}"), "ApiKey(…)");
        let config = ProviderConfig::custom(Backend::OpenAiChat, "http://x", Some(key));
        assert!(!format!("{config:?}").contains("sk-secret"));
    }

    #[test]
    fn scripted_streams_replay_their_events() {
        let stream = ChatStream::from_events(vec![
            Ok(StreamEvent::TextDelta("hi".into())),
            Err(LlmError::Incomplete),
        ]);
        let events = futures::executor::block_on(stream.collect::<Vec<_>>());
        assert_eq!(
            events,
            [
                Ok(StreamEvent::TextDelta("hi".into())),
                Err(LlmError::Incomplete)
            ]
        );
    }
}
