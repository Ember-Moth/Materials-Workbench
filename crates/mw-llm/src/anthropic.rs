//! Anthropic Messages API, streaming only (design §6.2.3).
//!
//! Field names, events and headers follow the Claude API documentation as of
//! 2026-10-22 (API version `2026-10-21`). Content blocks this module does not
//! know are kept as raw JSON and replayed unchanged.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::capabilities::Dialect;
use crate::driver::{self, IMAGE_OMITTED, RequestContext, system_prompt};
use crate::error::ApiError;
use crate::sse::SseEvent;
use crate::{
    AssistantContent, AssistantTurn, Backend, ChatRequest, Compaction, Image, LlmError, Message,
    Origin, Reasoning, Refusal, StopReason, StreamEvent, ToolCall, ToolResult, ToolResultContent,
    Usage, UserContent,
};

pub(crate) const API_VERSION: &str = "2026-10-21";
/// On-demand compaction; also required on every request that carries a
/// compaction block.
const COMPACTION_BETA: &str = "compact-2026-09-04";

#[derive(Clone, Debug)]
pub(crate) struct Wire {
    dialect: Dialect,
}

impl Wire {
    pub fn new(dialect: Dialect) -> Self {
        Self { dialect }
    }
}

impl driver::Wire for Wire {
    type Decoder = Decoder;
    const BACKEND: Backend = Backend::Anthropic;
    const PATH: &'static str = "/v1/messages";

    fn body(&self, request: &ChatRequest, ctx: &RequestContext) -> Result<Value, LlmError> {
        let caps = &ctx.caps;
        let options = &request.options;
        let mut body = Map::new();
        body.insert("model".into(), json!(ctx.origin.model));
        body.insert("max_tokens".into(), json!(ctx.max_output_tokens));
        body.insert("stream".into(), json!(true));

        // Stable prefix first: tools, then system (design §6.2.3).
        let mut tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                let mut entry = json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.input_schema,
                });
                if caps.strict_tools {
                    entry["strict"] = json!(true);
                }
                if self.dialect.eager_input_streaming {
                    entry["eager_input_streaming"] = json!(true);
                }
                entry
            })
            .collect();
        let system = system_prompt(request, caps);
        let mut system_blocks = Vec::new();
        if !system.is_empty() {
            system_blocks.push(json!({ "type": "text", "text": system }));
        }
        if caps.explicit_cache {
            // One breakpoint at the end of the stable prefix, plus automatic
            // caching for the growing conversation.
            if let Some(last) = system_blocks.last_mut().or(tools.last_mut()) {
                last["cache_control"] = json!({ "type": "ephemeral" });
            }
            body.insert("cache_control".into(), json!({ "type": "ephemeral" }));
        }
        if !tools.is_empty() {
            body.insert("tools".into(), Value::Array(tools));
        }
        if !system_blocks.is_empty() {
            body.insert("system".into(), Value::Array(system_blocks));
        }
        body.insert("messages".into(), Value::Array(messages(request, ctx)));

        if caps.reasoning_summary && options.reasoning_summary {
            body.insert(
                "thinking".into(),
                json!({ "type": "adaptive", "display": "summarized" }),
            );
        }
        if let Some(effort) = options.effort.filter(|_| caps.effort) {
            body.insert("output_config".into(), json!({ "effort": effort.as_str() }));
        }
        if let Some(compaction) = &options.compaction {
            let mut config = json!({ "type": "summarize" });
            if let Some(instructions) = &compaction.instructions {
                config["instructions"] = json!(instructions);
            }
            body.insert("compaction".into(), config);
        }
        Ok(Value::Object(body))
    }

    fn headers(&self, body: &Value) -> Vec<(&'static str, String)> {
        let mut headers = vec![("anthropic-version", API_VERSION.to_owned())];
        if body.get("compaction").is_some() || carries_compaction_block(body) {
            headers.push(("anthropic-beta", COMPACTION_BETA.to_owned()));
        }
        headers
    }

    fn decoder(&self, ctx: &RequestContext) -> Decoder {
        Decoder::new(ctx.origin.clone())
    }
}

fn carries_compaction_block(body: &Value) -> bool {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .any(|block| block["type"] == "compaction")
}

fn messages(request: &ChatRequest, ctx: &RequestContext) -> Vec<Value> {
    let caps = &ctx.caps;
    let mut out = Vec::new();
    // A mid-conversation system message must follow a user message; one
    // written after an assistant turn waits for the next user message.
    let mut waiting: Vec<&str> = Vec::new();
    for message in &request.messages {
        match message {
            Message::User(content) => {
                out.push(
                    json!({ "role": "user", "content": user_blocks(content, caps.image_input) }),
                );
                for text in waiting.drain(..) {
                    out.push(json!({ "role": "system", "content": text }));
                }
            }
            Message::Assistant(turn) => {
                let blocks = assistant_blocks(turn, ctx);
                if !blocks.is_empty() {
                    out.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
            Message::System(text) if caps.mid_conversation_system => {
                if out.last().is_some_and(|last| last["role"] == "user") {
                    out.push(json!({ "role": "system", "content": text }));
                } else {
                    waiting.push(text);
                }
            }
            // Already appended to the system prompt.
            Message::System(_) => {}
        }
    }
    out.extend(
        waiting
            .into_iter()
            .map(|text| json!({ "role": "system", "content": text })),
    );
    out
}

fn image_block(image: &Image, allowed: bool) -> Value {
    if allowed {
        json!({
            "type": "image",
            "source": { "type": "base64", "media_type": image.media_type, "data": image.data },
        })
    } else {
        json!({ "type": "text", "text": IMAGE_OMITTED })
    }
}

fn user_blocks(content: &[UserContent], images: bool) -> Vec<Value> {
    // Tool results must come before anything else in the message.
    let results = content.iter().filter_map(|part| match part {
        UserContent::ToolResult(result) => Some(tool_result(result, images)),
        _ => None,
    });
    let others = content.iter().filter_map(|part| match part {
        UserContent::Text(text) => Some(json!({ "type": "text", "text": text })),
        UserContent::Image(image) => Some(image_block(image, images)),
        UserContent::ToolResult(_) => None,
    });
    results.chain(others).collect()
}

fn tool_result(result: &ToolResult, images: bool) -> Value {
    let content: Vec<Value> = result
        .content
        .iter()
        .map(|part| match part {
            ToolResultContent::Text(text) => json!({ "type": "text", "text": text }),
            ToolResultContent::Image(image) => image_block(image, images),
        })
        .collect();
    let mut block = json!({
        "type": "tool_result",
        "tool_use_id": result.call_id,
        "content": content,
    });
    if result.is_error {
        block["is_error"] = json!(true);
    }
    block
}

fn is_reasoning_block(block: &Value) -> bool {
    matches!(
        block["type"].as_str(),
        Some("thinking" | "redacted_thinking")
    )
}

/// The turn as sent back: the raw blocks to the model that wrote them,
/// otherwise only text and tool calls (design §6.2.2).
fn assistant_blocks(turn: &AssistantTurn, ctx: &RequestContext) -> Vec<Value> {
    if turn.replays_raw_to(&ctx.origin)
        && let Some(blocks) = turn.raw.as_array()
    {
        return blocks
            .iter()
            .filter(|block| ctx.caps.reasoning_replay || !is_reasoning_block(block))
            .cloned()
            .collect();
    }
    turn.content
        .iter()
        .filter_map(|block| match block {
            AssistantContent::Text(text) if !text.is_empty() => {
                Some(json!({ "type": "text", "text": text }))
            }
            AssistantContent::ToolCall(call) => Some(json!({
                "type": "tool_use",
                "id": call.id,
                "name": call.name,
                "input": tool_input(call),
            })),
            AssistantContent::Compaction(compaction) => Some(json!({
                "type": "text",
                "text": format!("Summary of the earlier conversation:\n\n{}", compaction.summary),
            })),
            _ => None,
        })
        .collect()
}

/// `tool_use.input` must be an object; arguments that did not parse are sent
/// back as `{}` alongside the error result that explains them.
fn tool_input(call: &ToolCall) -> Value {
    match &call.arguments {
        Some(value @ Value::Object(_)) => value.clone(),
        _ => json!({}),
    }
}

#[derive(Debug)]
enum Block {
    Text {
        text: String,
        citations: Vec<Value>,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
    /// Kept as received: redacted thinking, compaction, server tool blocks,
    /// and types added after this was written.
    Other(Value),
}

pub(crate) struct Decoder {
    origin: Origin,
    blocks: BTreeMap<u64, Block>,
    stop_reason: Option<String>,
    stop_details: Option<Value>,
    usage: Map<String, Value>,
    done: bool,
}

impl Decoder {
    fn new(origin: Origin) -> Self {
        Self {
            origin,
            blocks: BTreeMap::new(),
            stop_reason: None,
            stop_details: None,
            usage: Map::new(),
            done: false,
        }
    }

    /// Usage fields are cumulative; later values replace earlier ones.
    fn merge_usage(&mut self, usage: &Value) {
        if let Some(usage) = usage.as_object() {
            for (key, value) in usage {
                if !value.is_null() {
                    self.usage.insert(key.clone(), value.clone());
                }
            }
        }
    }

    fn start_block(&mut self, index: u64, block: &Value, out: &mut Vec<StreamEvent>) {
        let text = |key: &str| block[key].as_str().unwrap_or_default().to_owned();
        let decoded = match block["type"].as_str() {
            Some("text") => {
                let text = text("text");
                if !text.is_empty() {
                    out.push(StreamEvent::TextDelta(text.clone()));
                }
                Block::Text {
                    text,
                    citations: block["citations"].as_array().cloned().unwrap_or_default(),
                }
            }
            Some("thinking") => Block::Thinking {
                thinking: text("thinking"),
                signature: text("signature"),
            },
            Some("tool_use") => {
                let (id, name) = (text("id"), text("name"));
                out.push(StreamEvent::ToolCallStart {
                    id: id.clone(),
                    name: name.clone(),
                });
                // The start event carries `input: {}` as a placeholder.
                let json = match &block["input"] {
                    Value::Object(input) if !input.is_empty() => block["input"].to_string(),
                    _ => String::new(),
                };
                Block::ToolUse { id, name, json }
            }
            _ => Block::Other(block.clone()),
        };
        self.blocks.insert(index, decoded);
    }

    fn delta(&mut self, index: u64, delta: &Value, out: &mut Vec<StreamEvent>) {
        let piece = |key: &str| delta[key].as_str().unwrap_or_default();
        let Some(block) = self.blocks.get_mut(&index) else {
            log::debug!("anthropic: delta for unknown block {index}");
            return;
        };
        match (block, delta["type"].as_str()) {
            (Block::Text { text, .. }, Some("text_delta")) => {
                let piece = piece("text");
                text.push_str(piece);
                if !piece.is_empty() {
                    out.push(StreamEvent::TextDelta(piece.to_owned()));
                }
            }
            (Block::Text { citations, .. }, Some("citations_delta")) => {
                citations.push(delta["citation"].clone());
            }
            (Block::Thinking { thinking, .. }, Some("thinking_delta")) => {
                let piece = piece("thinking");
                thinking.push_str(piece);
                if !piece.is_empty() {
                    out.push(StreamEvent::ReasoningDelta(piece.to_owned()));
                }
            }
            (Block::Thinking { signature, .. }, Some("signature_delta")) => {
                signature.push_str(piece("signature"));
            }
            (Block::ToolUse { id, json, .. }, Some("input_json_delta")) => {
                let piece = piece("partial_json");
                json.push_str(piece);
                if !piece.is_empty() {
                    out.push(StreamEvent::ToolCallDelta {
                        id: id.clone(),
                        arguments: piece.to_owned(),
                    });
                }
            }
            (_, kind) => log::debug!("anthropic: ignoring {kind:?} delta for block {index}"),
        }
    }

    fn usage(&self) -> Usage {
        let count = |entry: &Value, key: &str| entry[key].as_u64().unwrap_or(0);
        let read = |entry: &Value| Usage {
            input_tokens: count(entry, "input_tokens"),
            output_tokens: count(entry, "output_tokens"),
            cache_read_tokens: count(entry, "cache_read_input_tokens"),
            cache_write_tokens: count(entry, "cache_creation_input_tokens"),
        };
        let usage = Value::Object(self.usage.clone());
        // With compaction or fallback the top level covers one attempt only;
        // `iterations` lists every billed call.
        match usage["iterations"].as_array() {
            Some(iterations) if !iterations.is_empty() => {
                let mut total = Usage::default();
                for iteration in iterations {
                    total += read(iteration);
                }
                total
            }
            _ => read(&usage),
        }
    }
}

fn stream_error(error: &Value) -> LlmError {
    LlmError::Api(ApiError {
        status: None,
        kind: error["type"].as_str().map(str::to_owned),
        message: error["message"]
            .as_str()
            .unwrap_or("unknown error")
            .to_owned(),
        ..ApiError::default()
    })
}

impl driver::Decoder for Decoder {
    fn event(&mut self, event: SseEvent, out: &mut Vec<StreamEvent>) -> Result<(), LlmError> {
        let data: Value = serde_json::from_str(&event.data)
            .map_err(|error| LlmError::Decode(format!("event {:?}: {error}", event.event)))?;
        let kind = data["type"].as_str().unwrap_or(&event.event);
        let index = data["index"].as_u64().unwrap_or(0);
        match kind {
            "message_start" => self.merge_usage(&data["message"]["usage"]),
            "content_block_start" => self.start_block(index, &data["content_block"], out),
            "content_block_delta" => self.delta(index, &data["delta"], out),
            "message_delta" => {
                let delta = &data["delta"];
                if let Some(reason) = delta["stop_reason"].as_str() {
                    self.stop_reason = Some(reason.to_owned());
                }
                let details = if delta["stop_details"].is_object() {
                    &delta["stop_details"]
                } else {
                    &data["stop_details"]
                };
                if details.is_object() {
                    self.stop_details = Some(details.clone());
                }
                self.merge_usage(&data["usage"]);
            }
            "message_stop" => self.done = true,
            "error" => return Err(stream_error(&data["error"])),
            // `content_block_stop`, `ping`, and event types added later.
            _ => {}
        }
        Ok(())
    }

    fn is_done(&self) -> bool {
        self.done
    }

    fn finish(self) -> Result<AssistantTurn, LlmError> {
        if !self.done && self.stop_reason.is_none() {
            return Err(LlmError::Incomplete);
        }
        let usage = self.usage();
        let mut content = Vec::new();
        let mut raw = Vec::new();
        for block in self.blocks.into_values() {
            match block {
                Block::Text { text, citations } => {
                    let mut entry = json!({ "type": "text", "text": text });
                    if !citations.is_empty() {
                        entry["citations"] = Value::Array(citations);
                    }
                    raw.push(entry);
                    content.push(AssistantContent::Text(text));
                }
                Block::Thinking {
                    thinking,
                    signature,
                } => {
                    raw.push(
                        json!({ "type": "thinking", "thinking": thinking, "signature": signature }),
                    );
                    content.push(AssistantContent::Reasoning(Reasoning { summary: thinking }));
                }
                Block::ToolUse { id, name, json } => {
                    let call = ToolCall::new(id, name, json);
                    raw.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": tool_input(&call),
                    }));
                    content.push(AssistantContent::ToolCall(call));
                }
                Block::Other(block) => {
                    match block["type"].as_str() {
                        Some("compaction") => {
                            content.push(AssistantContent::Compaction(Compaction {
                                summary: block["content"].as_str().unwrap_or_default().to_owned(),
                            }))
                        }
                        Some("redacted_thinking") => {
                            content.push(AssistantContent::Reasoning(Reasoning::default()));
                        }
                        _ => {}
                    }
                    raw.push(block);
                }
            }
        }
        let stop_reason = match self.stop_reason.as_deref() {
            Some("end_turn") => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens" | "model_context_window_exceeded") => StopReason::MaxTokens,
            Some("refusal") => {
                let details = self.stop_details.unwrap_or_default();
                let field = |key: &str| details[key].as_str().map(str::to_owned);
                StopReason::Refusal(Refusal {
                    category: field("category"),
                    explanation: field("explanation"),
                })
            }
            Some("compaction") => StopReason::Compacted,
            other => StopReason::Other(other.unwrap_or("unknown").to_owned()),
        };
        Ok(AssistantTurn {
            origin: self.origin,
            content,
            stop_reason,
            vendor_stop_reason: self.stop_reason,
            usage,
            raw: Value::Array(raw),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{Capabilities, EndpointProfile, endpoint_profile};
    use crate::driver::{Wire as _, decode};
    use crate::{CompactionRequest, Effort, RequestOptions, ToolDefinition};

    const ENDPOINT: &str = "https://api.anthropic.com";

    fn origin(model: &str) -> Origin {
        Origin {
            backend: Backend::Anthropic,
            endpoint: ENDPOINT.into(),
            model: model.into(),
        }
    }

    fn official() -> (Wire, RequestContext) {
        let profile = endpoint_profile("anthropic").unwrap();
        let ctx = RequestContext {
            origin: origin("claude-opus-5-5"),
            caps: profile.capabilities("claude-opus-5-5"),
            max_output_tokens: 64_000,
        };
        (Wire::new(profile.dialect), ctx)
    }

    fn decode_fixture(name: &str) -> (Vec<StreamEvent>, Result<AssistantTurn, LlmError>) {
        let sse = match name {
            "tool_use" => include_str!("../tests/fixtures/anthropic/tool_use.sse"),
            "refusal" => include_str!("../tests/fixtures/anthropic/refusal.sse"),
            "error" => include_str!("../tests/fixtures/anthropic/error.sse"),
            "unknown" => include_str!("../tests/fixtures/anthropic/unknown.sse"),
            "compaction" => include_str!("../tests/fixtures/anthropic/compaction.sse"),
            _ => unreachable!(),
        };
        decode(Decoder::new(origin("claude-opus-5-5")), sse)
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": { "site": { "type": "string" }, "cutoff_angstrom": { "type": "number" } },
            "required": ["site", "cutoff_angstrom"],
            "additionalProperties": false
        })
    }

    fn png() -> Image {
        Image::new("image/png", "iVBORw0KGgo=")
    }

    fn request() -> ChatRequest {
        let first = decode_fixture("tool_use").1.unwrap();
        let mut options = RequestOptions::new("claude-opus-5-5");
        options.effort = Some(Effort::High);
        let mut request = ChatRequest::new(options);
        request.system = "You are the Materials Workbench assistant.".into();
        request.tools = vec![ToolDefinition::new(
            "structure_neighbors",
            "Neighbors of a site.",
            schema(),
        )];
        request.messages = vec![
            Message::User(vec![
                UserContent::Text("Ti–O distances?".into()),
                UserContent::Image(png()),
            ]),
            Message::Assistant(Box::new(first)),
            Message::User(vec![
                UserContent::Text("(viewport attached)".into()),
                UserContent::ToolResult(ToolResult {
                    call_id: "toolu_01T1x1fJ34qAmk2tNTrN7Up6".into(),
                    content: vec![
                        ToolResultContent::Text("4 × 1.95 Å, 2 × 1.98 Å".into()),
                        ToolResultContent::Image(png()),
                    ],
                    is_error: false,
                }),
            ]),
            Message::System("Answer in Chinese from now on.".into()),
        ];
        request
    }

    #[test]
    fn request_body_for_the_official_api() {
        let (wire, ctx) = official();
        let body = wire.body(&request(), &ctx).unwrap();
        let replayed = decode_fixture("tool_use").1.unwrap().raw;
        assert_eq!(
            body,
            json!({
                "model": "claude-opus-5-5",
                "max_tokens": 64000,
                "stream": true,
                "cache_control": { "type": "ephemeral" },
                "tools": [{
                    "name": "structure_neighbors",
                    "description": "Neighbors of a site.",
                    "input_schema": schema(),
                    "strict": true,
                    "eager_input_streaming": true
                }],
                "system": [{
                    "type": "text",
                    "text": "You are the Materials Workbench assistant.",
                    "cache_control": { "type": "ephemeral" }
                }],
                "messages": [
                    { "role": "user", "content": [
                        { "type": "text", "text": "Ti–O distances?" },
                        { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo=" } }
                    ]},
                    { "role": "assistant", "content": replayed },
                    { "role": "user", "content": [
                        { "type": "tool_result", "tool_use_id": "toolu_01T1x1fJ34qAmk2tNTrN7Up6", "content": [
                            { "type": "text", "text": "4 × 1.95 Å, 2 × 1.98 Å" },
                            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo=" } }
                        ]},
                        { "type": "text", "text": "(viewport attached)" }
                    ]},
                    { "role": "system", "content": "Answer in Chinese from now on." }
                ],
                "thinking": { "type": "adaptive", "display": "summarized" },
                "output_config": { "effort": "high" }
            })
        );
        assert_eq!(
            wire.headers(&body),
            [("anthropic-version", "2026-10-21".to_owned())]
        );
    }

    #[test]
    fn conservative_endpoints_fall_back() {
        let profile = EndpointProfile::custom(Backend::Anthropic);
        let ctx = RequestContext {
            origin: origin("claude-opus-5-5"),
            caps: Capabilities::CONSERVATIVE,
            max_output_tokens: 8192,
        };
        let mut request = request();
        // Same origin as the replayed turn, but reasoning replay is off.
        request.options.effort = Some(Effort::Low);
        let body = Wire::new(profile.dialect).body(&request, &ctx).unwrap();
        assert!(body.get("cache_control").is_none());
        assert!(body.get("thinking").is_none() && body.get("output_config").is_none());
        assert_eq!(body["tools"][0].get("strict"), None);
        assert_eq!(body["tools"][0].get("eager_input_streaming"), None);
        assert_eq!(
            body["system"][0]["text"],
            "You are the Materials Workbench assistant.\n\nAnswer in Chinese from now on."
        );
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(
            messages.len(),
            3,
            "the system message is folded into the prompt"
        );
        assert_eq!(
            messages[0]["content"][1],
            json!({ "type": "text", "text": IMAGE_OMITTED })
        );
        let types: Vec<&str> = messages[1]["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|block| block["type"].as_str().unwrap())
            .collect();
        assert_eq!(types, ["text", "tool_use"], "thinking is dropped");
    }

    #[test]
    fn turns_from_other_models_lose_their_reasoning() {
        let (wire, ctx) = official();
        let mut request = request();
        let Message::Assistant(turn) = &mut request.messages[1] else {
            unreachable!()
        };
        turn.origin = origin("claude-sonnet-5-5");
        let body = wire.body(&request, &ctx).unwrap();
        assert_eq!(
            body["messages"][1]["content"],
            json!([
                { "type": "text", "text": "I'll look up the neighbors of Ti." },
                { "type": "tool_use", "id": "toolu_01T1x1fJ34qAmk2tNTrN7Up6", "name": "structure_neighbors",
                  "input": { "site": "Ti1", "cutoff_angstrom": 2.5 } }
            ])
        );
    }

    #[test]
    fn system_messages_wait_for_a_user_message() {
        let (wire, ctx) = official();
        let mut request = ChatRequest::new(RequestOptions::new("claude-opus-5-5"));
        let answer = AssistantTurn::new(
            origin("claude-opus-5-5"),
            vec![AssistantContent::Text("Done.".into())],
            StopReason::EndTurn,
        );
        request.messages = vec![
            Message::user_text("a"),
            Message::Assistant(Box::new(answer)),
            Message::System("Be brief.".into()),
            Message::user_text("b"),
        ];
        let body = wire.body(&request, &ctx).unwrap();
        let roles: Vec<&str> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["user", "assistant", "user", "system"]);
    }

    #[test]
    fn compaction_requests_and_blocks_need_the_beta() {
        let (wire, ctx) = official();
        let mut request = ChatRequest::new(RequestOptions::new("claude-opus-5-5"));
        request.messages = vec![Message::user_text("a")];
        request.options.compaction = Some(CompactionRequest {
            instructions: Some("Keep every lattice parameter.".into()),
        });
        let body = wire.body(&request, &ctx).unwrap();
        assert_eq!(
            body["compaction"],
            json!({ "type": "summarize", "instructions": "Keep every lattice parameter." })
        );
        assert_eq!(
            wire.headers(&body)[1],
            ("anthropic-beta", COMPACTION_BETA.to_owned())
        );

        let summary = decode_fixture("compaction").1.unwrap();
        let mut next = ChatRequest::new(RequestOptions::new("claude-opus-5-5"));
        next.messages = vec![
            Message::Assistant(Box::new(summary)),
            Message::user_text("go on"),
        ];
        let body = wire.body(&next, &ctx).unwrap();
        assert_eq!(body["messages"][0]["content"][0]["type"], "compaction");
        assert_eq!(wire.headers(&body).len(), 2);
    }

    #[test]
    fn decodes_thinking_text_and_a_tool_call() {
        let (events, turn) = decode_fixture("tool_use");
        assert_eq!(
            events,
            [
                StreamEvent::ReasoningDelta("The user wants the Ti–O distances".into()),
                StreamEvent::ReasoningDelta(" in rutile; query the neighbors.".into()),
                StreamEvent::TextDelta("I'll look up the ".into()),
                StreamEvent::TextDelta("neighbors of Ti.".into()),
                StreamEvent::ToolCallStart {
                    id: "toolu_01T1x1fJ34qAmk2tNTrN7Up6".into(),
                    name: "structure_neighbors".into()
                },
                StreamEvent::ToolCallDelta {
                    id: "toolu_01T1x1fJ34qAmk2tNTrN7Up6".into(),
                    arguments: "{\"site\": \"Ti1\",".into()
                },
                StreamEvent::ToolCallDelta {
                    id: "toolu_01T1x1fJ34qAmk2tNTrN7Up6".into(),
                    arguments: " \"cutoff_angstrom\": 2.5}".into()
                },
            ]
        );
        let turn = turn.unwrap();
        assert_eq!(turn.stop_reason, StopReason::ToolUse);
        assert!(turn.may_run_tools());
        assert_eq!(
            turn.usage,
            Usage {
                input_tokens: 2048,
                output_tokens: 89,
                cache_read_tokens: 12000,
                cache_write_tokens: 512
            }
        );
        let call = turn.tool_calls().next().unwrap();
        assert_eq!(
            call.arguments_raw,
            "{\"site\": \"Ti1\", \"cutoff_angstrom\": 2.5}"
        );
        assert_eq!(
            call.arguments,
            Some(json!({ "site": "Ti1", "cutoff_angstrom": 2.5 }))
        );
        assert_eq!(
            turn.content[0],
            AssistantContent::Reasoning(Reasoning {
                summary: "The user wants the Ti–O distances in rutile; query the neighbors.".into()
            })
        );
        assert_eq!(
            turn.raw[0],
            json!({
                "type": "thinking",
                "thinking": "The user wants the Ti–O distances in rutile; query the neighbors.",
                "signature": "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds"
            })
        );
    }

    #[test]
    fn decodes_a_refusal_with_its_category() {
        let (events, turn) = decode_fixture("refusal");
        assert_eq!(events, [StreamEvent::TextDelta("Here is how".into())]);
        let turn = turn.unwrap();
        assert_eq!(
            turn.stop_reason,
            StopReason::Refusal(Refusal {
                category: Some("cyber".into()),
                explanation: Some(
                    "This request was declined because it could enable cyber harm.".into()
                )
            })
        );
        assert!(!turn.may_run_tools());
    }

    #[test]
    fn a_mid_stream_error_ends_decoding() {
        let (events, turn) = decode_fixture("error");
        assert_eq!(events, [StreamEvent::TextDelta("Partial".into())]);
        let Err(error) = turn else {
            panic!("expected an error")
        };
        assert_eq!(
            error.to_string(),
            "stream error (overloaded_error): Overloaded"
        );
        assert!(error.is_retryable());
    }

    #[test]
    fn unknown_events_are_skipped_and_unknown_blocks_kept() {
        let (events, turn) = decode_fixture("unknown");
        assert_eq!(events, [StreamEvent::TextDelta("Rutile is TiO₂.".into())]);
        let turn = turn.unwrap();
        assert_eq!(turn.stop_reason, StopReason::EndTurn);
        let types: Vec<&str> = turn
            .raw
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            [
                "redacted_thinking",
                "server_tool_use",
                "future_block",
                "text"
            ]
        );
        assert_eq!(turn.raw[0]["data"], "EmwKAhgBEgy3va3pzix/LafPsn4a");
        assert_eq!(
            turn.raw[2],
            json!({ "type": "future_block", "payload": { "x": 1 } })
        );
        assert_eq!(turn.raw[3]["citations"][0]["cited_text"], "TiO2");
        assert_eq!(turn.text(), "Rutile is TiO₂.");
    }

    #[test]
    fn decodes_a_compaction_summary() {
        let (events, turn) = decode_fixture("compaction");
        assert!(events.is_empty());
        let turn = turn.unwrap();
        assert_eq!(turn.stop_reason, StopReason::Compacted);
        assert_eq!(
            turn.content,
            [AssistantContent::Compaction(Compaction {
                summary: "The user is checking Ti–O bond lengths in rutile.".into()
            })]
        );
        assert_eq!(turn.raw[0]["signature"], "EuYBCkQY");
        // The top-level counts are zero; the call is billed in iterations.
        assert_eq!(turn.usage.input_tokens, 144);
        assert_eq!(turn.usage.output_tokens, 276);
    }

    #[test]
    fn a_truncated_stream_is_incomplete() {
        let sse = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{}}}\n\n\
                   event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n";
        let (_, turn) = decode(Decoder::new(origin("claude-opus-5-5")), sse);
        assert_eq!(turn, Err(LlmError::Incomplete));
    }
}
