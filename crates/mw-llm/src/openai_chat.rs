//! OpenAI Chat Completions, for OpenAI and the services and local servers
//! that copy its API (design §6.2.4). The base URL decides which.
//!
//! Compatible services add fields of their own, such as `reasoning_content`.
//! Unknown fields of the streamed message are assembled into the turn's raw
//! message and sent back to the same service unchanged.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::capabilities::{Dialect, MaxTokensField};
use crate::driver::{self, IMAGE_OMITTED, RequestContext, system_prompt};
use crate::error::ApiError;
use crate::sse::SseEvent;
use crate::{
    AssistantContent, AssistantTurn, Backend, ChatRequest, Image, LlmError, Message, Origin,
    Reasoning, Refusal, StopReason, StreamEvent, ToolCall, ToolResult, Usage, UserContent,
};

/// Fields compatible services use for reasoning text.
const REASONING_FIELDS: &[&str] = &["reasoning_content", "reasoning"];
/// Reasoning fields dropped when reasoning is not sent back.
const REASONING_REPLAY_FIELDS: &[&str] = &["reasoning_content", "reasoning", "reasoning_details"];

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
    const BACKEND: Backend = Backend::OpenAiChat;
    const PATH: &'static str = "/chat/completions";

    fn body(&self, request: &ChatRequest, ctx: &RequestContext) -> Result<Value, LlmError> {
        let caps = &ctx.caps;
        let mut body = Map::new();
        body.insert("model".into(), json!(ctx.origin.model));
        body.insert("stream".into(), json!(true));
        if self.dialect.stream_usage {
            body.insert("stream_options".into(), json!({ "include_usage": true }));
        }
        let limit = match self.dialect.max_tokens_field {
            MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
            MaxTokensField::MaxTokens => "max_tokens",
        };
        body.insert(limit.into(), json!(ctx.max_output_tokens));
        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|tool| {
                    let mut function = json!({
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    });
                    if caps.strict_tools {
                        function["strict"] = json!(true);
                    }
                    json!({ "type": "function", "function": function })
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
            // Compatible servers may reject the field, so it is only sent
            // where it is known to work.
            if caps.parallel_tool_calls {
                body.insert("parallel_tool_calls".into(), json!(true));
            }
        }
        if let Some(effort) = request.options.effort.filter(|_| caps.effort) {
            body.insert("reasoning_effort".into(), json!(effort.as_str()));
        }
        body.insert(
            "messages".into(),
            Value::Array(messages(request, ctx, &self.dialect)),
        );
        Ok(Value::Object(body))
    }

    fn headers(&self, _body: &Value) -> Vec<(&'static str, String)> {
        Vec::new()
    }

    fn decoder(&self, ctx: &RequestContext) -> Decoder {
        Decoder::new(ctx.origin.clone())
    }
}

fn messages(request: &ChatRequest, ctx: &RequestContext, dialect: &Dialect) -> Vec<Value> {
    let caps = &ctx.caps;
    let mut out = Vec::new();
    let system = system_prompt(request, caps);
    if !system.is_empty() {
        out.push(json!({ "role": dialect.system_role, "content": system }));
    }
    for message in &request.messages {
        match message {
            Message::User(content) => user_messages(content, caps.image_input, &mut out),
            Message::Assistant(turn) => out.extend(assistant_message(turn, ctx)),
            Message::System(text) if caps.mid_conversation_system => {
                out.push(json!({ "role": dialect.system_role, "content": text }));
            }
            Message::System(_) => {}
        }
    }
    out
}

fn image_part(image: &Image, allowed: bool) -> Value {
    if allowed {
        json!({ "type": "image_url", "image_url": { "url": image.data_url() } })
    } else {
        json!({ "type": "text", "text": IMAGE_OMITTED })
    }
}

/// Tool results become `tool` messages, which must directly follow the
/// assistant message that called them. Tool messages carry text only, so
/// images from tool results move to the user message after them.
fn user_messages(content: &[UserContent], images: bool, out: &mut Vec<Value>) {
    let mut parts = Vec::new();
    for part in content {
        if let UserContent::ToolResult(result) = part {
            out.push(tool_message(result));
            if images {
                for image in result.images() {
                    parts.push(json!({
                        "type": "text",
                        "text": format!("Image from tool call {}:", result.call_id),
                    }));
                    parts.push(image_part(image, true));
                }
            }
        }
    }
    for part in content {
        match part {
            UserContent::Text(text) => parts.push(json!({ "type": "text", "text": text })),
            UserContent::Image(image) => parts.push(image_part(image, images)),
            UserContent::ToolResult(_) => {}
        }
    }
    let content = match parts.as_slice() {
        [] => return,
        // Plain strings are the most widely supported form.
        [single] if single["type"] == "text" => single["text"].clone(),
        _ => Value::Array(parts),
    };
    out.push(json!({ "role": "user", "content": content }));
}

fn tool_message(result: &ToolResult) -> Value {
    let mut text = result.joined_text();
    if result.is_error {
        text = format!("Error: {text}");
    }
    json!({ "role": "tool", "tool_call_id": result.call_id, "content": text })
}

fn assistant_message(turn: &AssistantTurn, ctx: &RequestContext) -> Option<Value> {
    if turn.replays_raw_to(&ctx.origin)
        && let Some(raw) = turn.raw.as_object()
    {
        let mut message = raw.clone();
        if !ctx.caps.reasoning_replay {
            for field in REASONING_REPLAY_FIELDS {
                message.remove(*field);
            }
        }
        return Some(Value::Object(message));
    }
    let mut text = String::new();
    let mut calls = Vec::new();
    for block in &turn.content {
        match block {
            AssistantContent::Text(piece) => text.push_str(piece),
            AssistantContent::Compaction(compaction) => {
                text.push_str("Summary of the earlier conversation:\n\n");
                text.push_str(&compaction.summary);
            }
            AssistantContent::ToolCall(call) => calls.push(tool_call(call)),
            _ => {}
        }
    }
    if text.is_empty() && calls.is_empty() {
        return None;
    }
    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() { Value::Null } else { json!(text) },
    });
    if !calls.is_empty() {
        message["tool_calls"] = Value::Array(calls);
    }
    Some(message)
}

fn tool_call(call: &ToolCall) -> Value {
    json!({
        "id": call.id,
        "type": "function",
        "function": { "name": call.name, "arguments": call.arguments_raw },
    })
}

#[derive(Debug, Default)]
struct CallBuilder {
    id: String,
    name: String,
    arguments: String,
    announced: bool,
}

pub(crate) struct Decoder {
    origin: Origin,
    text: String,
    refusal: String,
    reasoning: String,
    reasoning_field: Option<String>,
    calls: BTreeMap<u64, CallBuilder>,
    /// Non-standard message fields, assembled from the deltas.
    extras: Map<String, Value>,
    finish_reason: Option<String>,
    usage: Option<Value>,
    done: bool,
}

impl Decoder {
    fn new(origin: Origin) -> Self {
        Self {
            origin,
            text: String::new(),
            refusal: String::new(),
            reasoning: String::new(),
            reasoning_field: None,
            calls: BTreeMap::new(),
            extras: Map::new(),
            finish_reason: None,
            usage: None,
            done: false,
        }
    }

    fn delta(&mut self, delta: &Map<String, Value>, out: &mut Vec<StreamEvent>) {
        for (key, value) in delta {
            match (key.as_str(), value) {
                (_, Value::Null) | ("role", _) => {}
                ("content", Value::String(text)) => {
                    self.text.push_str(text);
                    if !text.is_empty() {
                        out.push(StreamEvent::TextDelta(text.clone()));
                    }
                }
                ("refusal", Value::String(text)) => self.refusal.push_str(text),
                (field, Value::String(text)) if REASONING_FIELDS.contains(&field) => {
                    self.reasoning.push_str(text);
                    self.reasoning_field.get_or_insert_with(|| field.to_owned());
                    if !text.is_empty() {
                        out.push(StreamEvent::ReasoningDelta(text.clone()));
                    }
                }
                ("tool_calls", Value::Array(calls)) => {
                    for (position, call) in calls.iter().enumerate() {
                        self.tool_call_delta(position as u64, call, out);
                    }
                }
                (field, value) => merge_extra(&mut self.extras, field, value),
            }
        }
    }

    fn tool_call_delta(&mut self, position: u64, delta: &Value, out: &mut Vec<StreamEvent>) {
        let index = delta["index"].as_u64().unwrap_or(position);
        let call = self.calls.entry(index).or_default();
        if let Some(id) = delta["id"].as_str().filter(|id| !id.is_empty())
            && call.id.is_empty()
        {
            call.id = id.to_owned();
        }
        let function = &delta["function"];
        if let Some(name) = function["name"].as_str().filter(|name| !name.is_empty())
            && call.name.is_empty()
        {
            call.name = name.to_owned();
        }
        let arguments = function["arguments"].as_str().unwrap_or_default();
        call.arguments.push_str(arguments);
        if call.announced {
            if !arguments.is_empty() {
                out.push(StreamEvent::ToolCallDelta {
                    id: call.id.clone(),
                    arguments: arguments.to_owned(),
                });
            }
        } else if !call.id.is_empty() && !call.name.is_empty() {
            call.announced = true;
            out.push(StreamEvent::ToolCallStart {
                id: call.id.clone(),
                name: call.name.clone(),
            });
            if !call.arguments.is_empty() {
                out.push(StreamEvent::ToolCallDelta {
                    id: call.id.clone(),
                    arguments: call.arguments.clone(),
                });
            }
        }
    }

    fn usage(&self) -> Usage {
        let Some(usage) = &self.usage else {
            return Usage::default();
        };
        let count = |value: &Value| value.as_u64().unwrap_or(0);
        let prompt = count(&usage["prompt_tokens"]);
        // DeepSeek reports cache hits in a field of its own.
        let cached = usage["prompt_tokens_details"]["cached_tokens"]
            .as_u64()
            .or_else(|| usage["prompt_cache_hit_tokens"].as_u64())
            .unwrap_or(0);
        Usage {
            input_tokens: prompt.saturating_sub(cached),
            output_tokens: count(&usage["completion_tokens"]),
            cache_read_tokens: cached,
            cache_write_tokens: 0,
        }
    }
}

/// Strings are streamed in pieces and concatenated; arrays are extended;
/// anything else keeps its latest value.
fn merge_extra(extras: &mut Map<String, Value>, field: &str, value: &Value) {
    match (extras.get_mut(field), value) {
        (Some(Value::String(existing)), Value::String(piece)) => existing.push_str(piece),
        (Some(Value::Array(existing)), Value::Array(items)) => {
            existing.extend(items.iter().cloned())
        }
        _ => {
            extras.insert(field.to_owned(), value.clone());
        }
    }
}

impl driver::Decoder for Decoder {
    fn event(&mut self, event: SseEvent, out: &mut Vec<StreamEvent>) -> Result<(), LlmError> {
        if event.data.trim() == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let chunk: Value = serde_json::from_str(&event.data)
            .map_err(|error| LlmError::Decode(format!("chunk: {error}")))?;
        if let Some(error) = chunk.get("error").filter(|error| !error.is_null()) {
            let text = |key: &str| error[key].as_str().map(str::to_owned);
            return Err(LlmError::Api(ApiError {
                kind: text("code").or_else(|| text("type")),
                message: text("message").unwrap_or_else(|| error.to_string()),
                ..ApiError::default()
            }));
        }
        if chunk["usage"].is_object() {
            self.usage = Some(chunk["usage"].clone());
        }
        // Only one choice is requested.
        if let Some(choice) = chunk["choices"]
            .as_array()
            .and_then(|choices| choices.first())
        {
            if let Some(delta) = choice["delta"].as_object() {
                self.delta(delta, out);
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                self.finish_reason = Some(reason.to_owned());
            }
            // Some services report usage on the choice instead.
            if choice["usage"].is_object() {
                self.usage = Some(choice["usage"].clone());
            }
        }
        Ok(())
    }

    fn is_done(&self) -> bool {
        self.done
    }

    fn finish(self) -> Result<AssistantTurn, LlmError> {
        if !self.done && self.finish_reason.is_none() {
            return Err(LlmError::Incomplete);
        }
        let usage = self.usage();
        let calls: Vec<ToolCall> = self
            .calls
            .into_iter()
            .map(|(index, call)| {
                let id = if call.id.is_empty() {
                    format!("call_{index}")
                } else {
                    call.id
                };
                ToolCall::new(id, call.name, call.arguments)
            })
            .collect();

        let mut raw = Map::new();
        raw.insert("role".into(), json!("assistant"));
        raw.insert(
            "content".into(),
            if self.text.is_empty() {
                Value::Null
            } else {
                json!(self.text)
            },
        );
        if !calls.is_empty() {
            raw.insert("tool_calls".into(), calls.iter().map(tool_call).collect());
        }
        if !self.refusal.is_empty() {
            raw.insert("refusal".into(), json!(self.refusal));
        }
        if let Some(field) = &self.reasoning_field {
            raw.insert(field.clone(), json!(self.reasoning));
        }
        for (key, value) in self.extras {
            raw.entry(key).or_insert(value);
        }

        let mut content = Vec::new();
        if !self.reasoning.is_empty() {
            content.push(AssistantContent::Reasoning(Reasoning {
                summary: self.reasoning,
            }));
        }
        if !self.text.is_empty() {
            content.push(AssistantContent::Text(self.text));
        }
        let has_calls = !calls.is_empty();
        content.extend(calls.into_iter().map(AssistantContent::ToolCall));

        let stop_reason = if !self.refusal.is_empty() {
            StopReason::Refusal(Refusal {
                category: None,
                explanation: Some(self.refusal),
            })
        } else {
            match self.finish_reason.as_deref() {
                Some("tool_calls" | "function_call") => StopReason::ToolUse,
                // Some servers report `stop` after tool calls.
                Some("stop") | None if has_calls => StopReason::ToolUse,
                Some("stop") | None => StopReason::EndTurn,
                Some("length") => StopReason::MaxTokens,
                Some("content_filter") => StopReason::Refusal(Refusal {
                    category: Some("content_filter".into()),
                    explanation: None,
                }),
                Some(other) => StopReason::Other(other.to_owned()),
            }
        };
        Ok(AssistantTurn {
            origin: self.origin,
            content,
            stop_reason,
            vendor_stop_reason: self.finish_reason,
            usage,
            raw: Value::Object(raw),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{Capabilities, EndpointProfile, endpoint_profile};
    use crate::driver::{Wire as _, decode};
    use crate::{Effort, RequestOptions, ToolDefinition, ToolResultContent};

    fn origin(endpoint: &str, model: &str) -> Origin {
        Origin {
            backend: Backend::OpenAiChat,
            endpoint: endpoint.into(),
            model: model.into(),
        }
    }

    fn decode_fixture(
        name: &str,
        origin: Origin,
    ) -> (Vec<StreamEvent>, Result<AssistantTurn, LlmError>) {
        let sse = match name {
            "tool_calls" => include_str!("../tests/fixtures/openai_chat/tool_calls.sse"),
            "reasoning" => include_str!("../tests/fixtures/openai_chat/reasoning.sse"),
            "refusal" => include_str!("../tests/fixtures/openai_chat/refusal.sse"),
            "error" => include_str!("../tests/fixtures/openai_chat/error.sse"),
            "length" => include_str!("../tests/fixtures/openai_chat/length.sse"),
            _ => unreachable!(),
        };
        decode(Decoder::new(origin), sse)
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": { "formula": { "type": "string" } },
            "required": ["formula"],
            "additionalProperties": false
        })
    }

    fn request(model: &str) -> ChatRequest {
        let mut options = RequestOptions::new(model);
        options.effort = Some(Effort::Medium);
        let mut request = ChatRequest::new(options);
        request.system = "You are the Materials Workbench assistant.".into();
        request.tools = vec![ToolDefinition::new(
            "structure_search",
            "Search structures.",
            schema(),
        )];
        request.messages = vec![Message::User(vec![
            UserContent::Text("Find rutile.".into()),
            UserContent::Image(Image::new("image/png", "AAAA")),
        ])];
        request
    }

    #[test]
    fn request_body_for_openai() {
        let profile = endpoint_profile("openai-chat").unwrap();
        let ctx = RequestContext {
            origin: origin("https://api.openai.com/v1", "gpt-x"),
            caps: profile.capabilities("gpt-x"),
            max_output_tokens: 32_000,
        };
        let mut request = request("gpt-x");
        let (_, turn) = decode_fixture("tool_calls", ctx.origin.clone());
        request
            .messages
            .push(Message::Assistant(Box::new(turn.unwrap())));
        request.messages.push(Message::User(vec![
            UserContent::ToolResult(ToolResult {
                call_id: "call_abc".into(),
                content: vec![
                    ToolResultContent::Text("1 match".into()),
                    ToolResultContent::Image(Image::new("image/png", "BBBB")),
                ],
                is_error: false,
            }),
            UserContent::ToolResult(ToolResult::error("call_def", "unknown space group")),
        ]));
        request.messages.push(Message::System("Be brief.".into()));
        let body = Wire::new(profile.dialect).body(&request, &ctx).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "gpt-x",
                "stream": true,
                "stream_options": { "include_usage": true },
                "max_completion_tokens": 32000,
                "tools": [{ "type": "function", "function": {
                    "name": "structure_search", "description": "Search structures.",
                    "parameters": schema(), "strict": true
                }}],
                "parallel_tool_calls": true,
                "reasoning_effort": "medium",
                "messages": [
                    { "role": "developer", "content": "You are the Materials Workbench assistant." },
                    { "role": "user", "content": [
                        { "type": "text", "text": "Find rutile." },
                        { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } }
                    ]},
                    { "role": "assistant", "content": "Searching.", "tool_calls": [
                        { "id": "call_abc", "type": "function", "function": { "name": "structure_search", "arguments": "{\"formula\":\"TiO2\"}" } },
                        { "id": "call_def", "type": "function", "function": { "name": "structure_search", "arguments": "{\"formula\":\"SrTiO3\"}" } }
                    ]},
                    { "role": "tool", "tool_call_id": "call_abc", "content": "1 match" },
                    { "role": "tool", "tool_call_id": "call_def", "content": "Error: unknown space group" },
                    { "role": "user", "content": [
                        { "type": "text", "text": "Image from tool call call_abc:" },
                        { "type": "image_url", "image_url": { "url": "data:image/png;base64,BBBB" } }
                    ]},
                    { "role": "developer", "content": "Be brief." }
                ]
            })
        );
    }

    #[test]
    fn compatible_services_get_the_conservative_form() {
        let profile = endpoint_profile("deepseek").unwrap();
        let ctx = RequestContext {
            origin: origin("https://api.deepseek.com", "deepseek-reasoner"),
            caps: profile.capabilities("deepseek-reasoner"),
            max_output_tokens: 8192,
        };
        let mut request = request("deepseek-reasoner");
        let (_, turn) = decode_fixture("reasoning", ctx.origin.clone());
        request
            .messages
            .push(Message::Assistant(Box::new(turn.unwrap())));
        request.messages.push(Message::System("Be brief.".into()));
        request.messages.push(Message::user_text("Thanks"));
        let body = Wire::new(profile.dialect).body(&request, &ctx).unwrap();
        assert_eq!(body["max_tokens"], 8192);
        for absent in [
            "max_completion_tokens",
            "stream_options",
            "parallel_tool_calls",
            "reasoning_effort",
        ] {
            assert!(body.get(absent).is_none(), "{absent}");
        }
        assert!(body["tools"][0]["function"].get("strict").is_none());
        assert_eq!(
            body["messages"],
            json!([
                { "role": "system", "content": "You are the Materials Workbench assistant.\n\nBe brief." },
                { "role": "user", "content": [
                    { "type": "text", "text": "Find rutile." },
                    { "type": "text", "text": IMAGE_OMITTED }
                ]},
                // Same service and model: the raw message, minus reasoning.
                { "role": "assistant", "content": "Rutile is tetragonal.", "x_vendor_trace": "abc" },
                { "role": "user", "content": "Thanks" }
            ])
        );
    }

    #[test]
    fn turns_from_elsewhere_are_rebuilt() {
        let profile = EndpointProfile::custom(Backend::OpenAiChat);
        let ctx = RequestContext {
            origin: origin("http://localhost:8000/v1", "qwen3"),
            caps: Capabilities::CONSERVATIVE,
            max_output_tokens: 4096,
        };
        let mut request = ChatRequest::new(RequestOptions::new("qwen3"));
        let (_, turn) = decode_fixture(
            "reasoning",
            origin("https://api.deepseek.com", "deepseek-reasoner"),
        );
        request.messages = vec![
            Message::user_text("Hi"),
            Message::Assistant(Box::new(turn.unwrap())),
        ];
        let body = Wire::new(profile.dialect).body(&request, &ctx).unwrap();
        assert_eq!(
            body["messages"][1],
            json!({ "role": "assistant", "content": "Rutile is tetragonal." })
        );
    }

    #[test]
    fn decodes_parallel_tool_calls() {
        let (events, turn) =
            decode_fixture("tool_calls", origin("https://api.openai.com/v1", "gpt-x"));
        assert_eq!(
            events,
            [
                StreamEvent::TextDelta("Searching.".into()),
                StreamEvent::ToolCallStart {
                    id: "call_abc".into(),
                    name: "structure_search".into()
                },
                StreamEvent::ToolCallDelta {
                    id: "call_abc".into(),
                    arguments: "{\"formula\"".into()
                },
                StreamEvent::ToolCallDelta {
                    id: "call_abc".into(),
                    arguments: ":\"TiO2\"}".into()
                },
                StreamEvent::ToolCallStart {
                    id: "call_def".into(),
                    name: "structure_search".into()
                },
                StreamEvent::ToolCallDelta {
                    id: "call_def".into(),
                    arguments: "{\"formula\":\"SrTiO3\"}".into()
                },
            ]
        );
        let turn = turn.unwrap();
        assert_eq!(turn.stop_reason, StopReason::ToolUse);
        assert_eq!(turn.tool_calls().count(), 2);
        assert_eq!(
            turn.usage,
            Usage {
                input_tokens: 400,
                output_tokens: 42,
                cache_read_tokens: 1024,
                cache_write_tokens: 0
            }
        );
    }

    #[test]
    fn decodes_reasoning_content_and_vendor_fields() {
        let (events, turn) = decode_fixture(
            "reasoning",
            origin("https://api.deepseek.com", "deepseek-reasoner"),
        );
        assert_eq!(
            events,
            [
                StreamEvent::ReasoningDelta("Rutile: P4₂/mnm".into()),
                StreamEvent::ReasoningDelta(", tetragonal.".into()),
                StreamEvent::TextDelta("Rutile is tetragonal.".into()),
            ]
        );
        let turn = turn.unwrap();
        assert_eq!(turn.stop_reason, StopReason::EndTurn);
        assert_eq!(
            turn.raw["reasoning_content"],
            "Rutile: P4₂/mnm, tetragonal."
        );
        assert_eq!(turn.raw["x_vendor_trace"], "abc");
        assert_eq!(
            turn.usage,
            Usage {
                input_tokens: 90,
                output_tokens: 20,
                cache_read_tokens: 10,
                cache_write_tokens: 0
            }
        );
    }

    #[test]
    fn decodes_refusals_filters_and_truncation() {
        let (_, turn) = decode_fixture("refusal", origin("https://api.openai.com/v1", "gpt-x"));
        assert_eq!(
            turn.unwrap().stop_reason,
            StopReason::Refusal(Refusal {
                category: None,
                explanation: Some("I can't help with that.".into())
            })
        );
        let (events, turn) = decode_fixture("length", origin("http://localhost:11434/v1", "llama"));
        assert_eq!(events, [StreamEvent::TextDelta("The lattice".into())]);
        let turn = turn.unwrap();
        assert_eq!(turn.stop_reason, StopReason::MaxTokens);
        assert!(!turn.may_run_tools());
    }

    #[test]
    fn a_mid_stream_error_ends_decoding() {
        let (events, turn) = decode_fixture("error", origin("http://localhost:8000/v1", "m"));
        assert_eq!(events, [StreamEvent::TextDelta("Hel".into())]);
        assert_eq!(
            turn.unwrap_err().to_string(),
            "stream error (server_error): The server had an error while processing your request."
        );
    }

    #[test]
    fn content_filter_and_missing_ids() {
        let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"function\":{\"name\":\"t\",\"arguments\":\"{}\"}}]}}]}\n\n\
                   data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"content_filter\"}]}\n\n";
        let (events, turn) = decode(Decoder::new(origin("x", "m")), sse);
        assert!(events.is_empty(), "a call without an id is not announced");
        let turn = turn.unwrap();
        assert_eq!(turn.tool_calls().next().unwrap().id, "call_0");
        assert_eq!(
            turn.stop_reason,
            StopReason::Refusal(Refusal {
                category: Some("content_filter".into()),
                explanation: None
            })
        );
    }
}
