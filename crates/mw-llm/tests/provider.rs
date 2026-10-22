//! The real HTTP path against a throwaway server on 127.0.0.1: what goes on
//! the wire, and that dropping a stream closes the connection. No request
//! leaves the machine.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use futures::StreamExt;
use mw_llm::{
    ApiKey, Backend, ChatRequest, EndpointProfile, Message, ProviderConfig, RequestOptions,
    RetryPolicy, Runtime, StopReason, StreamEvent, endpoint_profile,
};
use serde_json::Value;

/// A request as the server saw it.
struct Received {
    request_line: String,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn read_request(stream: &mut TcpStream) -> Received {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    let length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.parse().unwrap())
        .unwrap();
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    Received {
        request_line: request_line.trim_end().to_owned(),
        headers,
        body: serde_json::from_slice(&body).unwrap(),
    }
}

/// Serves one request with `respond`, and reports what it received.
fn serve_once(
    respond: impl FnOnce(&mut TcpStream) + Send + 'static,
) -> (String, mpsc::Receiver<Received>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let received = read_request(&mut stream);
        tx.send(received).unwrap();
        respond(&mut stream);
    });
    (url, rx)
}

const SSE_HEAD: &str =
    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";

fn config(profile: &EndpointProfile, url: &str, key: Option<&str>) -> ProviderConfig {
    let mut config = ProviderConfig::new(profile, key.map(ApiKey::new));
    config.base_url = url.to_owned();
    config.retry = RetryPolicy::NEVER;
    config
}

fn hello(model: &str) -> ChatRequest {
    let mut request = ChatRequest::new(RequestOptions::new(model));
    request.messages.push(Message::user_text("Hello"));
    request
}

#[test]
fn anthropic_request_on_the_wire() {
    let (url, received) = serve_once(|stream| {
        stream.write_all(SSE_HEAD.as_bytes()).unwrap();
        stream
            .write_all(include_str!("fixtures/anthropic/tool_use.sse").as_bytes())
            .unwrap();
    });
    let runtime = Runtime::new().unwrap();
    let profile = endpoint_profile("anthropic").unwrap();
    let provider = mw_llm::connect(config(profile, &url, Some("sk-ant-test")), &runtime).unwrap();
    let events: Vec<_> =
        futures::executor::block_on(provider.stream(hello("claude-opus-5-5")).collect());

    let request = received.recv().unwrap();
    assert_eq!(request.request_line, "POST /v1/messages HTTP/1.1");
    assert_eq!(request.header("x-api-key"), Some("sk-ant-test"));
    assert_eq!(request.header("anthropic-version"), Some("2026-10-21"));
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(request.header("authorization"), None);
    assert_eq!(request.body["model"], "claude-opus-5-5");
    assert_eq!(request.body["max_tokens"], 128_000);

    let Some(Ok(StreamEvent::Done(turn))) = events.last() else {
        panic!("expected a finished turn, got {events:?}");
    };
    assert_eq!(turn.stop_reason, StopReason::ToolUse);
    assert_eq!(turn.origin, provider.origin("claude-opus-5-5"));
    assert_eq!(turn.origin.endpoint, url);
}

#[test]
fn openai_compatible_request_on_the_wire() {
    let (url, received) = serve_once(|stream| {
        stream.write_all(SSE_HEAD.as_bytes()).unwrap();
        stream
            .write_all(include_str!("fixtures/openai_chat/reasoning.sse").as_bytes())
            .unwrap();
    });
    let runtime = Runtime::new().unwrap();
    let provider = mw_llm::connect(
        config(
            &EndpointProfile::custom(Backend::OpenAiChat),
            &format!("{url}/v1/"),
            Some("sk-test"),
        ),
        &runtime,
    )
    .unwrap();
    let events: Vec<_> =
        futures::executor::block_on(provider.stream(hello("local-model")).collect());

    let request = received.recv().unwrap();
    assert_eq!(request.request_line, "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(request.header("authorization"), Some("Bearer sk-test"));
    assert_eq!(request.body["max_tokens"], 8192);
    let Some(Ok(StreamEvent::Done(turn))) = events.last() else {
        panic!("expected a finished turn, got {events:?}");
    };
    assert_eq!(turn.text(), "Rutile is tetragonal.");
}

#[test]
fn http_errors_reach_the_caller() {
    let (url, _received) = serve_once(|stream| {
        let body = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"},"request_id":"req_9"}"#;
        write!(
            stream,
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let runtime = Runtime::new().unwrap();
    let profile = endpoint_profile("anthropic").unwrap();
    let provider = mw_llm::connect(config(profile, &url, Some("bad")), &runtime).unwrap();
    let events: Vec<_> =
        futures::executor::block_on(provider.stream(hello("claude-opus-5-5")).collect());
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].as_ref().unwrap_err().to_string(),
        "HTTP 401 (authentication_error): invalid x-api-key [request req_9]"
    );
}

#[test]
fn dropping_the_stream_closes_the_connection() {
    let (closed_tx, closed_rx) = mpsc::channel();
    let (url, _received) = serve_once(move |stream| {
        stream.write_all(SSE_HEAD.as_bytes()).unwrap();
        stream
            .write_all(b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n")
            .unwrap();
        // Keep the response open; the read returns once the client is gone.
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut buffer = [0; 16];
        let closed = matches!(stream.read(&mut buffer), Ok(0));
        closed_tx.send(closed).unwrap();
    });
    let runtime = Runtime::new().unwrap();
    let profile = endpoint_profile("anthropic").unwrap();
    let provider = mw_llm::connect(config(profile, &url, Some("k")), &runtime).unwrap();
    let mut stream = provider.stream(hello("claude-opus-5-5"));
    let first = futures::executor::block_on(stream.next());
    assert_eq!(first, Some(Ok(StreamEvent::TextDelta("Hi".into()))));
    drop(stream);
    assert_eq!(closed_rx.recv_timeout(Duration::from_secs(10)), Ok(true));
}

#[test]
fn configuration_errors_arrive_on_the_stream() {
    let runtime = Runtime::new().unwrap();
    let profile = endpoint_profile("anthropic").unwrap();
    let provider = mw_llm::connect(config(profile, "http://127.0.0.1:9", None), &runtime).unwrap();
    let mut request = hello("claude-opus-5-5");
    request.tools.push(mw_llm::ToolDefinition::new(
        "structure.summary",
        "",
        serde_json::json!({ "type": "object" }),
    ));
    let events: Vec<_> = futures::executor::block_on(provider.stream(request).collect());
    let message = events[0].as_ref().unwrap_err().to_string();
    assert!(message.starts_with("invalid request: tool definitions outside the strict subset"));
    assert!(message.contains("structure.summary: tool names must match"));

    let missing = endpoint_profile("azure-openai").unwrap();
    let error = mw_llm::connect(ProviderConfig::new(missing, None), &runtime)
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "invalid request: no base URL configured for Azure OpenAI (Responses)"
    );
}
