//! What the three backends share: sending the request, retrying, reading the
//! event stream and handing events to the caller.
//!
//! Each backend supplies a [`Wire`] (request body and headers) and a
//! [`Decoder`] (SSE events to [`StreamEvent`]s and the final turn). Decoders
//! are plain state machines, tested by feeding them event-stream text.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{SinkExt, StreamExt};
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use crate::capabilities::Auth;
use crate::error::ApiError;
use crate::provider::ChatStream;
use crate::sse::{SseEvent, SseParser};
use crate::{
    AssistantTurn, Backend, Capabilities, ChatProvider, ChatRequest, LlmError, Origin,
    ProviderConfig, Runtime, StreamEvent, schema,
};

/// When to send a failed request again (design §6.2.3). Only failures before
/// anything reached the caller are retried; after that the error is passed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// Backoff without a `retry-after` header, doubled each retry.
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    /// Give up rather than wait longer than this for a `retry-after`.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
            max_retry_after: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    pub const NEVER: Self = Self {
        max_retries: 0,
        initial_backoff: Duration::ZERO,
        max_backoff: Duration::ZERO,
        max_retry_after: Duration::ZERO,
    };

    /// How long to wait before retry number `retries_done + 1`, or `None` to
    /// give up.
    pub fn delay(&self, retries_done: u32, retry_after: Option<Duration>) -> Option<Duration> {
        if retries_done >= self.max_retries {
            return None;
        }
        if let Some(wait) = retry_after {
            return (wait <= self.max_retry_after).then_some(wait);
        }
        let backoff = self
            .initial_backoff
            .saturating_mul(1 << retries_done.min(16))
            .min(self.max_backoff);
        Some(jitter(backoff))
    }
}

/// Scales `delay` by a factor in [0.75, 1.25) so that clients that failed
/// together don't retry together.
fn jitter(delay: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.subsec_nanos());
    delay.mul_f64(0.75 + f64::from(nanos % 1000) / 2000.0)
}

/// `retry-after-ms` (OpenAI) or `retry-after` in seconds. HTTP dates are not
/// used by either API and fall back to backoff.
pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let number = |name: &str| -> Option<f64> {
        headers
            .get(name)?
            .to_str()
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && *value >= 0.0)
    };
    number("retry-after-ms")
        .map(|ms| Duration::from_secs_f64(ms / 1000.0))
        .or_else(|| number("retry-after").map(Duration::from_secs_f64))
}

/// Builds an [`ApiError`] from an error response. Both vendors put an
/// `error` object with `message` and `type` (OpenAI also `code`) in the body.
pub(crate) fn api_error(status: Option<u16>, headers: &HeaderMap, body: &[u8]) -> ApiError {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let error = parsed.as_ref().and_then(|body| body.get("error"));
    let field = |name: &str| {
        error
            .and_then(|error| error.get(name))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let message = field("message").unwrap_or_else(|| {
        let text = String::from_utf8_lossy(body);
        text.chars().take(500).collect::<String>().trim().to_owned()
    });
    let request_id = ["request-id", "x-request-id"]
        .iter()
        .find_map(|name| headers.get(*name)?.to_str().ok().map(str::to_owned))
        .or_else(|| {
            parsed
                .as_ref()?
                .get("request_id")?
                .as_str()
                .map(str::to_owned)
        });
    ApiError {
        status,
        kind: field("code").or_else(|| field("type")),
        message,
        request_id,
        retry_after: retry_after(headers),
    }
}

/// Turns one backend's event stream into [`StreamEvent`]s and a turn.
pub(crate) trait Decoder: Send + 'static {
    /// Handles one event, pushing what the caller should see to `out`.
    fn event(&mut self, event: SseEvent, out: &mut Vec<StreamEvent>) -> Result<(), LlmError>;
    /// The terminal event has arrived; the rest of the body is ignored.
    fn is_done(&self) -> bool;
    /// Called once the stream has ended.
    fn finish(self) -> Result<AssistantTurn, LlmError>;
}

/// Settings resolved for one request.
#[derive(Clone, Debug)]
pub(crate) struct RequestContext {
    pub origin: Origin,
    pub caps: Capabilities,
    pub max_output_tokens: u32,
}

/// One backend's request format.
pub(crate) trait Wire: Clone + Send + Sync + 'static {
    type Decoder: Decoder;
    const BACKEND: Backend;
    /// Appended to the base URL.
    const PATH: &'static str;

    fn body(&self, request: &ChatRequest, ctx: &RequestContext) -> Result<Value, LlmError>;
    /// Headers besides authentication and content type, given the body.
    fn headers(&self, body: &Value) -> Vec<(&'static str, String)>;
    fn decoder(&self, ctx: &RequestContext) -> Self::Decoder;
}

/// The system prompt to send. Without mid-conversation system messages, the
/// operator instructions are appended to it in order (design §6.2.7).
pub(crate) fn system_prompt(request: &ChatRequest, caps: &Capabilities) -> String {
    let mut prompt = request.system.clone();
    if !caps.mid_conversation_system {
        for message in &request.messages {
            if let crate::Message::System(text) = message {
                if !prompt.is_empty() {
                    prompt.push_str("\n\n");
                }
                prompt.push_str(text);
            }
        }
    }
    prompt
}

/// Stands in for an image the model cannot receive.
pub(crate) const IMAGE_OMITTED: &str = "[An image was omitted: this model does not accept images.]";

/// A [`ChatProvider`] that speaks one [`Wire`] over HTTP.
pub(crate) struct HttpProvider<W> {
    wire: W,
    config: ProviderConfig,
    endpoint: String,
    client: reqwest::Client,
    runtime: Runtime,
}

impl<W: Wire> HttpProvider<W> {
    pub fn new(wire: W, config: ProviderConfig, runtime: &Runtime) -> Result<Self, LlmError> {
        let endpoint = config.base_url.trim().trim_end_matches('/').to_owned();
        if endpoint.is_empty() {
            return Err(LlmError::InvalidRequest(format!(
                "no base URL configured for {}",
                config.profile.name
            )));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(config.timeouts.connect)
            .read_timeout(config.timeouts.idle)
            .user_agent(concat!(
                "materials-workbench-mw-llm/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|error| LlmError::InvalidRequest(format!("HTTP client: {error}")))?;
        Ok(Self {
            wire,
            config,
            endpoint,
            client,
            runtime: runtime.clone(),
        })
    }

    fn context(&self, model: &str, requested: Option<u32>) -> RequestContext {
        let caps = self.capabilities(model);
        RequestContext {
            origin: self.origin(model),
            caps,
            max_output_tokens: requested.map_or(caps.max_output_tokens, |n| {
                n.clamp(1, caps.max_output_tokens.max(1))
            }),
        }
    }

    fn headers(&self, body: &Value) -> Result<HeaderMap, LlmError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );
        if let Some(key) = &self.config.api_key {
            let (name, value) = match self.config.profile.auth {
                Auth::None => (None, String::new()),
                Auth::Bearer => (
                    Some(header::AUTHORIZATION),
                    format!("Bearer {}", key.expose()),
                ),
                Auth::XApiKey => (
                    Some(HeaderName::from_static("x-api-key")),
                    key.expose().to_owned(),
                ),
                Auth::ApiKeyHeader => (
                    Some(HeaderName::from_static("api-key")),
                    key.expose().to_owned(),
                ),
            };
            if let Some(name) = name {
                let mut value = HeaderValue::from_str(&value).map_err(|_| {
                    LlmError::InvalidRequest("the API key contains invalid characters".into())
                })?;
                value.set_sensitive(true);
                headers.insert(name, value);
            }
        }
        for (name, value) in self.wire.headers(body) {
            let value = HeaderValue::from_str(&value)
                .map_err(|_| LlmError::InvalidRequest(format!("invalid {name} header")))?;
            headers.insert(HeaderName::from_static(name), value);
        }
        Ok(headers)
    }

    fn start(&self, request: ChatRequest) -> Result<ChatStream, LlmError> {
        let model = request.options.model.trim();
        if model.is_empty() {
            return Err(LlmError::InvalidRequest("no model given".into()));
        }
        let ctx = self.context(model, request.options.max_output_tokens);
        if request.options.compaction.is_some() && !ctx.caps.server_compaction {
            return Err(LlmError::InvalidRequest(format!(
                "{model} at {} does not support server-side compaction",
                self.endpoint
            )));
        }
        if let Err(issues) = schema::check_tools(&request.tools, ctx.caps.strict_tools) {
            let issues: Vec<String> = issues.iter().map(ToString::to_string).collect();
            return Err(LlmError::InvalidRequest(format!(
                "tool definitions outside the strict subset: {}",
                issues.join("; ")
            )));
        }
        let body = self.wire.body(&request, &ctx)?;
        let headers = self.headers(&body)?;
        let body = serde_json::to_vec(&body)
            .map_err(|error| LlmError::InvalidRequest(error.to_string()))?;
        let transport = ReqwestTransport {
            client: self.client.clone(),
            url: format!("{}{}", self.endpoint, W::PATH),
            headers,
            body: Bytes::from(body),
        };
        log::debug!(
            "{} request to {} (model {}, {} messages, {} tools)",
            W::BACKEND.id(),
            transport.url,
            model,
            request.messages.len(),
            request.tools.len()
        );
        let (tx, rx) = mpsc::channel(32);
        let wire = self.wire.clone();
        let retry = self.config.retry;
        let task = self
            .runtime
            .spawn(async move { run(&transport, || wire.decoder(&ctx), retry, tx).await });
        Ok(ChatStream::new(rx.boxed(), task))
    }
}

impl<W: Wire> ChatProvider for HttpProvider<W> {
    fn backend(&self) -> Backend {
        W::BACKEND
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        self.config
            .profile
            .capabilities(model)
            .with_overrides(&self.config.overrides)
    }

    fn stream(&self, request: ChatRequest) -> ChatStream {
        self.start(request).unwrap_or_else(ChatStream::failed)
    }
}

pub(crate) struct HttpResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: BoxStream<'static, Result<Bytes, LlmError>>,
}

/// Sends the request; called again for every retry.
pub(crate) trait Transport: Send + Sync {
    fn send(&self) -> BoxFuture<'static, Result<HttpResponse, LlmError>>;
}

struct ReqwestTransport {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    body: Bytes,
}

fn transport_error(error: reqwest::Error) -> LlmError {
    LlmError::Transport(error.without_url().to_string())
}

impl Transport for ReqwestTransport {
    fn send(&self) -> BoxFuture<'static, Result<HttpResponse, LlmError>> {
        let request = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .body(self.body.clone());
        Box::pin(async move {
            let response = request.send().await.map_err(transport_error)?;
            Ok(HttpResponse {
                status: response.status().as_u16(),
                headers: response.headers().clone(),
                body: response
                    .bytes_stream()
                    .map(|chunk| chunk.map_err(transport_error))
                    .boxed(),
            })
        })
    }
}

type Sender = mpsc::Sender<Result<StreamEvent, LlmError>>;

enum Outcome {
    Finished,
    /// The caller dropped the stream.
    Abandoned,
    Failed {
        error: LlmError,
        forwarded: bool,
    },
}

/// Runs one request to the end, retrying as [`RetryPolicy`] allows.
pub(crate) async fn run<D: Decoder>(
    transport: &dyn Transport,
    decoder: impl Fn() -> D,
    retry: RetryPolicy,
    mut tx: Sender,
) {
    let mut retries = 0;
    loop {
        match attempt(transport, decoder(), &mut tx).await {
            Outcome::Finished | Outcome::Abandoned => return,
            Outcome::Failed { error, forwarded } => {
                let wait = if !forwarded && error.is_retryable() {
                    let retry_after = match &error {
                        LlmError::Api(api) => api.retry_after,
                        _ => None,
                    };
                    retry.delay(retries, retry_after)
                } else {
                    None
                };
                let Some(wait) = wait else {
                    let _ = tx.send(Err(error)).await;
                    return;
                };
                retries += 1;
                log::warn!("retrying in {wait:?} (retry {retries}): {error}");
                tokio::time::sleep(wait).await;
            }
        }
    }
}

/// At most this much of an error body is read.
const MAX_ERROR_BODY: usize = 64 * 1024;

async fn read_body(mut body: BoxStream<'static, Result<Bytes, LlmError>>) -> Vec<u8> {
    let mut bytes = Vec::new();
    while let Some(Ok(chunk)) = body.next().await {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_ERROR_BODY {
            break;
        }
    }
    bytes
}

async fn attempt<D: Decoder>(
    transport: &dyn Transport,
    mut decoder: D,
    tx: &mut Sender,
) -> Outcome {
    let failed = |error, forwarded| Outcome::Failed { error, forwarded };
    let response = match transport.send().await {
        Ok(response) => response,
        Err(error) => return failed(error, false),
    };
    if !(200..300).contains(&response.status) {
        let body = read_body(response.body).await;
        let error = api_error(Some(response.status), &response.headers, &body);
        return failed(LlmError::Api(error), false);
    }
    let content_type = response
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if content_type.starts_with("application/json") {
        // Some compatible servers answer a streaming request with a plain
        // JSON error and status 200.
        let body = read_body(response.body).await;
        let mut error = api_error(None, &response.headers, &body);
        if error.message.is_empty() {
            error.message = "expected an event stream, got JSON".into();
        }
        return failed(LlmError::Api(error), false);
    }

    let mut body = response.body;
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    let mut forwarded = false;
    'read: loop {
        let (events, ended) = match body.next().await {
            Some(Ok(chunk)) => (parser.push(&chunk), false),
            Some(Err(error)) => return failed(error, forwarded),
            None => (parser.finish().into_iter().collect(), true),
        };
        for event in events {
            let result = decoder.event(event, &mut out);
            for item in out.drain(..) {
                if tx.send(Ok(item)).await.is_err() {
                    return Outcome::Abandoned;
                }
                forwarded = true;
            }
            if let Err(error) = result {
                return failed(error, forwarded);
            }
            if decoder.is_done() {
                break 'read;
            }
        }
        if ended {
            break;
        }
    }
    match decoder.finish() {
        Ok(turn) => {
            if tx
                .send(Ok(StreamEvent::Done(Box::new(turn))))
                .await
                .is_err()
            {
                return Outcome::Abandoned;
            }
            Outcome::Finished
        }
        Err(error) => failed(error, forwarded),
    }
}

/// Feeds `sse` to `decoder` in small chunks, the way the network might split
/// it, and collects what the caller would see.
#[cfg(test)]
pub(crate) fn decode<D: Decoder>(
    mut decoder: D,
    sse: &str,
) -> (Vec<StreamEvent>, Result<AssistantTurn, LlmError>) {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    let mut events: Vec<SseEvent> = Vec::new();
    for chunk in sse.as_bytes().chunks(7) {
        events.extend(parser.push(chunk));
    }
    events.extend(parser.finish());
    for event in events {
        if let Err(error) = decoder.event(event, &mut out) {
            return (out, Err(error));
        }
        if decoder.is_done() {
            break;
        }
    }
    (out, decoder.finish())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::{StopReason, Usage};

    /// `text:…` emits text, `error:<kind>` fails, `done` ends.
    struct TestDecoder {
        done: bool,
        text: String,
    }

    impl Decoder for TestDecoder {
        fn event(&mut self, event: SseEvent, out: &mut Vec<StreamEvent>) -> Result<(), LlmError> {
            if let Some(text) = event.data.strip_prefix("text:") {
                self.text.push_str(text);
                out.push(StreamEvent::TextDelta(text.into()));
            } else if let Some(kind) = event.data.strip_prefix("error:") {
                return Err(LlmError::Api(ApiError {
                    kind: Some(kind.into()),
                    message: "boom".into(),
                    ..ApiError::default()
                }));
            } else if event.data == "done" {
                self.done = true;
            }
            Ok(())
        }

        fn is_done(&self) -> bool {
            self.done
        }

        fn finish(self) -> Result<AssistantTurn, LlmError> {
            if !self.done {
                return Err(LlmError::Incomplete);
            }
            let origin = Origin {
                backend: Backend::OpenAiChat,
                endpoint: "test".into(),
                model: "m".into(),
            };
            let mut turn = AssistantTurn::new(origin, Vec::new(), StopReason::EndTurn);
            turn.vendor_stop_reason = Some(self.text);
            turn.usage = Usage::default();
            Ok(turn)
        }
    }

    /// Status, headers, and body chunks.
    type Response = (u16, Vec<(&'static str, &'static str)>, Vec<&'static str>);

    struct Scripted {
        responses: Mutex<VecDeque<Response>>,
        calls: AtomicUsize,
    }

    impl Scripted {
        fn new(responses: Vec<Response>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl Transport for Scripted {
        fn send(&self) -> BoxFuture<'static, Result<HttpResponse, LlmError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let next = self.responses.lock().unwrap().pop_front();
            Box::pin(async move {
                let (status, headers, chunks) =
                    next.ok_or(LlmError::Transport("no more responses".into()))?;
                let mut map = HeaderMap::new();
                for (name, value) in headers {
                    map.insert(
                        HeaderName::from_static(name),
                        HeaderValue::from_static(value),
                    );
                }
                Ok(HttpResponse {
                    status,
                    headers: map,
                    body: futures::stream::iter(
                        chunks
                            .into_iter()
                            .map(|chunk| Ok(Bytes::from_static(chunk.as_bytes()))),
                    )
                    .boxed(),
                })
            })
        }
    }

    const FAST: RetryPolicy = RetryPolicy {
        max_retries: 2,
        initial_backoff: Duration::ZERO,
        max_backoff: Duration::ZERO,
        max_retry_after: Duration::from_secs(1),
    };

    fn drive(transport: &Scripted, retry: RetryPolicy) -> Vec<Result<StreamEvent, LlmError>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let (tx, rx) = mpsc::channel(32);
        runtime.block_on(async {
            run(
                transport,
                || TestDecoder {
                    done: false,
                    text: String::new(),
                },
                retry,
                tx,
            )
            .await;
        });
        futures::executor::block_on(rx.collect())
    }

    fn texts(items: &[Result<StreamEvent, LlmError>]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                Ok(StreamEvent::TextDelta(text)) => format!("text {text}"),
                Ok(StreamEvent::Done(turn)) => {
                    format!("done {}", turn.vendor_stop_reason.as_deref().unwrap_or(""))
                }
                Ok(other) => format!("{other:?}"),
                Err(error) => format!("error {error}"),
            })
            .collect()
    }

    const STREAM: (&str, &str) = ("content-type", "text/event-stream");

    #[test]
    fn retries_overload_before_any_output() {
        let transport = Scripted::new(vec![
            (
                529,
                vec![("retry-after", "0")],
                vec![
                    r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
                ],
            ),
            (200, vec![STREAM], vec!["data: error:overloaded_error\n\n"]),
            (
                200,
                vec![STREAM],
                vec!["data: text:he", "llo\n\ndata: done\n\n"],
            ),
        ]);
        let items = drive(&transport, FAST);
        assert_eq!(texts(&items), ["text hello", "done hello"]);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn errors_after_output_go_to_the_caller() {
        let transport = Scripted::new(vec![(
            200,
            vec![STREAM],
            vec!["data: text:partial\n\ndata: error:overloaded_error\n\n"],
        )]);
        let items = drive(&transport, FAST);
        assert_eq!(
            texts(&items),
            [
                "text partial",
                "error stream error (overloaded_error): boom"
            ]
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn client_errors_are_not_retried() {
        let transport = Scripted::new(vec![(
            400,
            vec![("request-id", "req_1")],
            vec![
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"},"request_id":"req_1"}"#,
            ],
        )]);
        let items = drive(&transport, FAST);
        assert_eq!(
            texts(&items),
            ["error HTTP 400 (invalid_request_error): bad [request req_1]"]
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn gives_up_after_the_retry_budget() {
        let rate_limited = (
            429,
            vec![],
            vec![
                r#"{"error":{"message":"slow down","type":"requests","code":"rate_limit_exceeded"}}"#,
            ],
        );
        let transport = Scripted::new(vec![
            rate_limited.clone(),
            rate_limited.clone(),
            rate_limited,
        ]);
        let items = drive(&transport, FAST);
        assert_eq!(
            texts(&items),
            ["error HTTP 429 (rate_limit_exceeded): slow down"]
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_long_retry_after_is_not_waited_out() {
        let transport = Scripted::new(vec![(429, vec![("retry-after", "3600")], vec!["{}"])]);
        let items = drive(&transport, FAST);
        assert_eq!(items.len(), 1);
        let Err(LlmError::Api(error)) = &items[0] else {
            panic!("expected an API error");
        };
        assert_eq!(error.retry_after, Some(Duration::from_secs(3600)));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_stream_that_just_stops_is_incomplete() {
        let transport = Scripted::new(vec![(200, vec![STREAM], vec!["data: text:a\n\n"])]);
        let items = drive(&transport, RetryPolicy::NEVER);
        assert_eq!(
            texts(&items),
            ["text a", "error the response ended before it was complete"]
        );
    }

    #[test]
    fn json_instead_of_a_stream_is_an_error() {
        let transport = Scripted::new(vec![(
            200,
            vec![("content-type", "application/json")],
            vec![r#"{"error":{"message":"model not found","type":"invalid_request_error"}}"#],
        )]);
        let items = drive(&transport, RetryPolicy::NEVER);
        assert_eq!(
            texts(&items),
            ["error stream error (invalid_request_error): model not found"]
        );
    }

    #[test]
    fn retry_delays() {
        let policy = RetryPolicy::default();
        assert_eq!(
            policy.delay(0, Some(Duration::from_secs(5))),
            Some(Duration::from_secs(5))
        );
        assert_eq!(policy.delay(0, Some(Duration::from_secs(61))), None);
        assert_eq!(policy.delay(3, None), None);
        let first = policy.delay(0, None).unwrap();
        assert!(first >= Duration::from_millis(750) && first < Duration::from_millis(1250));
        let late = policy.delay(2, None).unwrap();
        assert!(late >= Duration::from_secs(3) && late < Duration::from_secs(5));

        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("2"));
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(2)));
        headers.insert("retry-after-ms", HeaderValue::from_static("250"));
        assert_eq!(retry_after(&headers), Some(Duration::from_millis(250)));
    }
}
