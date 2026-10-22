//! OpenAI Responses in stateless mode (design §6.2.4): `store: false`, the
//! whole history in `input`, and reasoning sent back as the encrypted items
//! the service returned.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value, json};

use crate::driver::{self, IMAGE_OMITTED, RequestContext, system_prompt};
use crate::error::ApiError;
use crate::sse::SseEvent;
use crate::{
    AssistantContent, AssistantTurn, Backend, ChatRequest, Image, LlmError, Message, Origin,
    Reasoning, Refusal, StopReason, StreamEvent, ToolCall, ToolResult, ToolResultContent, Usage,
    UserContent,
};

#[derive(Clone, Debug)]
pub(crate) struct Wire;

impl driver::Wire for Wire {
    type Decoder = Decoder;
    const BACKEND: Backend = Backend::OpenAiResponses;
    const PATH: &'static str = "/responses";

    fn body(&self, request: &ChatRequest, ctx: &RequestContext) -> Result<Value, LlmError> {
        let caps = &ctx.caps;
        let options = &request.options;
        let mut body = Map::new();
        body.insert("model".into(), json!(ctx.origin.model));
        body.insert("stream".into(), json!(true));
        body.insert("store".into(), json!(false));
        let instructions = system_prompt(request, caps);
        if !instructions.is_empty() {
            body.insert("instructions".into(), json!(instructions));
        }
        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|tool| {
                    // Explicit either way: left out, Responses tries to make
                    // the schema strict on its own.
                    json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                        "strict": caps.strict_tools,
                    })
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
            body.insert(
                "parallel_tool_calls".into(),
                json!(caps.parallel_tool_calls),
            );
        }
        body.insert("max_output_tokens".into(), json!(ctx.max_output_tokens));
        let mut reasoning = Map::new();
        if let Some(effort) = options.effort.filter(|_| caps.effort) {
            reasoning.insert("effort".into(), json!(effort.as_str()));
        }
        if caps.reasoning_summary && options.reasoning_summary {
            reasoning.insert("summary".into(), json!("auto"));
        }
        if !reasoning.is_empty() {
            body.insert("reasoning".into(), Value::Object(reasoning));
        }
        if caps.reasoning_replay {
            // Returned by default when `store` is false; still requested
            // explicitly, which the API accepts for compatibility.
            body.insert("include".into(), json!(["reasoning.encrypted_content"]));
        }
        body.insert("input".into(), Value::Array(input(request, ctx)));
        Ok(Value::Object(body))
    }

    fn headers(&self, _body: &Value) -> Vec<(&'static str, String)> {
        Vec::new()
    }

    fn decoder(&self, ctx: &RequestContext) -> Decoder {
        Decoder::new(ctx.origin.clone())
    }
}

fn input(request: &ChatRequest, ctx: &RequestContext) -> Vec<Value> {
    let caps = &ctx.caps;
    let mut items = Vec::new();
    for message in &request.messages {
        match message {
            Message::User(content) => user_items(content, caps.image_input, &mut items),
            Message::Assistant(turn) => items.extend(assistant_items(turn, ctx)),
            Message::System(text) if caps.mid_conversation_system => {
                items.push(json!({ "type": "message", "role": "developer", "content": text }));
            }
            Message::System(_) => {}
        }
    }
    items
}

fn image_part(image: &Image, allowed: bool) -> Value {
    if allowed {
        json!({ "type": "input_image", "image_url": image.data_url(), "detail": "auto" })
    } else {
        json!({ "type": "input_text", "text": IMAGE_OMITTED })
    }
}

fn user_items(content: &[UserContent], images: bool, items: &mut Vec<Value>) {
    for part in content {
        if let UserContent::ToolResult(result) = part {
            items.push(tool_output(result, images));
        }
    }
    let parts: Vec<Value> = content
        .iter()
        .filter_map(|part| match part {
            UserContent::Text(text) => Some(json!({ "type": "input_text", "text": text })),
            UserContent::Image(image) => Some(image_part(image, images)),
            UserContent::ToolResult(_) => None,
        })
        .collect();
    if !parts.is_empty() {
        items.push(json!({ "type": "message", "role": "user", "content": parts }));
    }
}

fn tool_output(result: &ToolResult, images: bool) -> Value {
    let prefix = if result.is_error { "Error: " } else { "" };
    let output = if result.images().next().is_none() {
        json!(format!("{prefix}{}", result.joined_text()))
    } else {
        let mut parts = Vec::new();
        if result.is_error {
            parts.push(json!({ "type": "input_text", "text": "Error:" }));
        }
        for part in &result.content {
            parts.push(match part {
                ToolResultContent::Text(text) => json!({ "type": "input_text", "text": text }),
                ToolResultContent::Image(image) => image_part(image, images),
            });
        }
        Value::Array(parts)
    };
    json!({ "type": "function_call_output", "call_id": result.call_id, "output": output })
}

fn assistant_items(turn: &AssistantTurn, ctx: &RequestContext) -> Vec<Value> {
    if turn.replays_raw_to(&ctx.origin)
        && let Some(items) = turn.raw.as_array()
    {
        return items
            .iter()
            .filter(|item| ctx.caps.reasoning_replay || item["type"] != "reasoning")
            .cloned()
            .collect();
    }
    turn.content
        .iter()
        .filter_map(|block| match block {
            AssistantContent::Text(text) if !text.is_empty() => {
                Some(json!({ "type": "message", "role": "assistant", "content": text }))
            }
            AssistantContent::Compaction(compaction) => Some(json!({
                "type": "message",
                "role": "assistant",
                "content": format!("Summary of the earlier conversation:\n\n{}", compaction.summary),
            })),
            AssistantContent::ToolCall(call) => Some(json!({
                "type": "function_call",
                "call_id": call.id,
                "name": call.name,
                "arguments": call.arguments_raw,
            })),
            _ => None,
        })
        .collect()
}

pub(crate) struct Decoder {
    origin: Origin,
    /// Finished output items by `output_index`.
    items: BTreeMap<u64, Value>,
    /// `item_id` to `call_id`, for argument deltas.
    call_ids: HashMap<String, String>,
    response: Option<Value>,
    done: bool,
}

impl Decoder {
    fn new(origin: Origin) -> Self {
        Self {
            origin,
            items: BTreeMap::new(),
            call_ids: HashMap::new(),
            response: None,
            done: false,
        }
    }
}

fn api_error(error: &Value) -> LlmError {
    let text = |key: &str| error[key].as_str().map(str::to_owned);
    LlmError::Api(ApiError {
        kind: text("code").or_else(|| text("type")),
        message: text("message").unwrap_or_else(|| "unknown error".into()),
        ..ApiError::default()
    })
}

impl driver::Decoder for Decoder {
    fn event(&mut self, event: SseEvent, out: &mut Vec<StreamEvent>) -> Result<(), LlmError> {
        let data: Value = serde_json::from_str(&event.data)
            .map_err(|error| LlmError::Decode(format!("event {:?}: {error}", event.event)))?;
        let kind = data["type"].as_str().unwrap_or(&event.event);
        let delta = || data["delta"].as_str().unwrap_or_default().to_owned();
        match kind {
            "response.output_item.added" => {
                let item = &data["item"];
                if item["type"] == "function_call" {
                    let call_id = item["call_id"].as_str().unwrap_or_default().to_owned();
                    if let Some(item_id) = item["id"].as_str() {
                        self.call_ids.insert(item_id.to_owned(), call_id.clone());
                    }
                    out.push(StreamEvent::ToolCallStart {
                        id: call_id,
                        name: item["name"].as_str().unwrap_or_default().to_owned(),
                    });
                }
            }
            "response.output_text.delta" => {
                let text = delta();
                if !text.is_empty() {
                    out.push(StreamEvent::TextDelta(text));
                }
            }
            "response.reasoning_summary_part.added" => {
                if data["summary_index"].as_u64().unwrap_or(0) > 0 {
                    out.push(StreamEvent::ReasoningDelta("\n\n".into()));
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let text = delta();
                if !text.is_empty() {
                    out.push(StreamEvent::ReasoningDelta(text));
                }
            }
            "response.function_call_arguments.delta" => {
                let item_id = data["item_id"].as_str().unwrap_or_default();
                let id = self.call_ids.get(item_id).cloned().unwrap_or_default();
                let arguments = delta();
                if !arguments.is_empty() {
                    out.push(StreamEvent::ToolCallDelta { id, arguments });
                }
            }
            "response.output_item.done" => {
                let index = data["output_index"]
                    .as_u64()
                    .unwrap_or(self.items.len() as u64);
                self.items.insert(index, data["item"].clone());
            }
            "response.completed" | "response.incomplete" => {
                self.response = Some(data["response"].clone());
                self.done = true;
            }
            "response.failed" => return Err(api_error(&data["response"]["error"])),
            "error" => return Err(api_error(&data)),
            // Lifecycle events, refusal deltas (read from the finished item),
            // and event types added later.
            _ => {}
        }
        Ok(())
    }

    fn is_done(&self) -> bool {
        self.done
    }

    fn finish(self) -> Result<AssistantTurn, LlmError> {
        let Some(response) = self.response.filter(|_| self.done) else {
            return Err(LlmError::Incomplete);
        };
        let items: Vec<Value> = if self.items.is_empty() {
            response["output"].as_array().cloned().unwrap_or_default()
        } else {
            self.items.into_values().collect()
        };

        let mut content = Vec::new();
        let mut refusal = String::new();
        for item in &items {
            match item["type"].as_str() {
                Some("message") => {
                    for part in item["content"].as_array().into_iter().flatten() {
                        match part["type"].as_str() {
                            Some("output_text") => content.push(AssistantContent::Text(
                                part["text"].as_str().unwrap_or_default().to_owned(),
                            )),
                            Some("refusal") => {
                                refusal.push_str(part["refusal"].as_str().unwrap_or_default());
                            }
                            _ => {}
                        }
                    }
                }
                Some("reasoning") => {
                    let summary: Vec<&str> = item["summary"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|part| part["text"].as_str())
                        .collect();
                    content.push(AssistantContent::Reasoning(Reasoning {
                        summary: summary.join("\n\n"),
                    }));
                }
                Some("function_call") => content.push(AssistantContent::ToolCall(ToolCall::new(
                    item["call_id"].as_str().unwrap_or_default(),
                    item["name"].as_str().unwrap_or_default(),
                    item["arguments"].as_str().unwrap_or_default(),
                ))),
                _ => {}
            }
        }

        let status = response["status"].as_str().unwrap_or("completed");
        let incomplete = response["incomplete_details"]["reason"].as_str();
        let has_calls = content
            .iter()
            .any(|block| matches!(block, AssistantContent::ToolCall(_)));
        let stop_reason = match (status, incomplete) {
            _ if !refusal.is_empty() => StopReason::Refusal(Refusal {
                category: None,
                explanation: Some(refusal),
            }),
            ("completed", _) if has_calls => StopReason::ToolUse,
            ("completed", _) => StopReason::EndTurn,
            ("incomplete", Some("max_output_tokens")) => StopReason::MaxTokens,
            ("incomplete", Some("content_filter")) => StopReason::Refusal(Refusal {
                category: Some("content_filter".into()),
                explanation: None,
            }),
            (status, reason) => StopReason::Other(reason.unwrap_or(status).to_owned()),
        };
        let vendor_stop_reason = Some(match incomplete {
            Some(reason) => format!("{status}: {reason}"),
            None => status.to_owned(),
        });

        let usage = &response["usage"];
        let count = |value: &Value| value.as_u64().unwrap_or(0);
        let cached = count(&usage["input_tokens_details"]["cached_tokens"]);
        Ok(AssistantTurn {
            origin: self.origin,
            content,
            stop_reason,
            vendor_stop_reason,
            usage: Usage {
                input_tokens: count(&usage["input_tokens"]).saturating_sub(cached),
                output_tokens: count(&usage["output_tokens"]),
                cache_read_tokens: cached,
                cache_write_tokens: 0,
            },
            raw: Value::Array(items),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{Capabilities, endpoint_profile};
    use crate::driver::{Wire as _, decode};
    use crate::{Effort, RequestOptions, ToolDefinition};

    const OPENAI: &str = "https://api.openai.com/v1";

    fn origin(model: &str) -> Origin {
        Origin {
            backend: Backend::OpenAiResponses,
            endpoint: OPENAI.into(),
            model: model.into(),
        }
    }

    fn decode_fixture(name: &str) -> (Vec<StreamEvent>, Result<AssistantTurn, LlmError>) {
        let sse = match name {
            "tool_call" => include_str!("../tests/fixtures/openai_responses/tool_call.sse"),
            "refusal" => include_str!("../tests/fixtures/openai_responses/refusal.sse"),
            "incomplete" => include_str!("../tests/fixtures/openai_responses/incomplete.sse"),
            "failed" => include_str!("../tests/fixtures/openai_responses/failed.sse"),
            "error" => include_str!("../tests/fixtures/openai_responses/error.sse"),
            _ => unreachable!(),
        };
        decode(Decoder::new(origin("gpt-x")), sse)
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": { "formula": { "type": "string" } },
            "required": ["formula"],
            "additionalProperties": false
        })
    }

    fn context(caps: Capabilities, model: &str) -> RequestContext {
        RequestContext {
            origin: origin(model),
            caps,
            max_output_tokens: 32_000,
        }
    }

    fn conversation(model: &str) -> ChatRequest {
        let mut options = RequestOptions::new(model);
        options.effort = Some(Effort::High);
        let mut request = ChatRequest::new(options);
        request.system = "You are the Materials Workbench assistant.".into();
        request.tools = vec![ToolDefinition::new(
            "structure_search",
            "Search structures.",
            schema(),
        )];
        request.messages = vec![
            Message::user_text("Find rutile."),
            Message::Assistant(Box::new(decode_fixture("tool_call").1.unwrap())),
            Message::User(vec![UserContent::ToolResult(ToolResult {
                call_id: "call_k3".into(),
                content: vec![
                    ToolResultContent::Text("1 match".into()),
                    ToolResultContent::Image(Image::new("image/png", "AAAA")),
                ],
                is_error: false,
            })]),
            Message::System("Be brief.".into()),
        ];
        request
    }

    #[test]
    fn stateless_request_body() {
        let caps = endpoint_profile("openai").unwrap().capabilities("gpt-x");
        let body = Wire
            .body(&conversation("gpt-x"), &context(caps, "gpt-x"))
            .unwrap();
        let raw = decode_fixture("tool_call").1.unwrap().raw;
        assert_eq!(
            body,
            json!({
                "model": "gpt-x",
                "stream": true,
                "store": false,
                "instructions": "You are the Materials Workbench assistant.",
                "tools": [{
                    "type": "function", "name": "structure_search", "description": "Search structures.",
                    "parameters": schema(), "strict": true
                }],
                "parallel_tool_calls": true,
                "max_output_tokens": 32000,
                "reasoning": { "effort": "high", "summary": "auto" },
                "include": ["reasoning.encrypted_content"],
                "input": [
                    { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Find rutile." }] },
                    raw[0], raw[1], raw[2],
                    { "type": "function_call_output", "call_id": "call_k3", "output": [
                        { "type": "input_text", "text": "1 match" },
                        { "type": "input_image", "image_url": "data:image/png;base64,AAAA", "detail": "auto" }
                    ]},
                    { "type": "message", "role": "developer", "content": "Be brief." }
                ]
            })
        );
        assert_eq!(raw[0]["type"], "reasoning");
        assert_eq!(raw[0]["encrypted_content"], "gAAAAABo-encrypted");
    }

    #[test]
    fn other_models_get_text_and_calls_only() {
        let caps = endpoint_profile("openai").unwrap().capabilities("gpt-y");
        let body = Wire
            .body(&conversation("gpt-y"), &context(caps, "gpt-y"))
            .unwrap();
        assert_eq!(
            body["input"].as_array().unwrap()[1..3],
            [
                json!({ "type": "message", "role": "assistant", "content": "Let me search." }),
                json!({ "type": "function_call", "call_id": "call_k3", "name": "structure_search", "arguments": "{\"formula\":\"TiO2\"}" }),
            ]
        );
    }

    #[test]
    fn conservative_capabilities() {
        let body = Wire
            .body(
                &conversation("gpt-x"),
                &context(Capabilities::CONSERVATIVE, "gpt-x"),
            )
            .unwrap();
        for absent in ["reasoning", "include"] {
            assert!(body.get(absent).is_none(), "{absent}");
        }
        assert_eq!(body["tools"][0]["strict"], false);
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(
            body["instructions"],
            "You are the Materials Workbench assistant.\n\nBe brief."
        );
        let types: Vec<&str> = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            [
                "message",
                "message",
                "function_call",
                "function_call_output"
            ],
            "reasoning dropped"
        );
        assert_eq!(
            body["input"][3]["output"][1],
            json!({ "type": "input_text", "text": IMAGE_OMITTED })
        );
    }

    #[test]
    fn decodes_reasoning_summary_text_and_a_call() {
        let (events, turn) = decode_fixture("tool_call");
        assert_eq!(
            events,
            [
                StreamEvent::ReasoningDelta("Need the structure".into()),
                StreamEvent::ReasoningDelta(" database.".into()),
                StreamEvent::ReasoningDelta("\n\n".into()),
                StreamEvent::ReasoningDelta("Search TiO2.".into()),
                StreamEvent::TextDelta("Let me ".into()),
                StreamEvent::TextDelta("search.".into()),
                StreamEvent::ToolCallStart {
                    id: "call_k3".into(),
                    name: "structure_search".into()
                },
                StreamEvent::ToolCallDelta {
                    id: "call_k3".into(),
                    arguments: "{\"formula\":".into()
                },
                StreamEvent::ToolCallDelta {
                    id: "call_k3".into(),
                    arguments: "\"TiO2\"}".into()
                },
            ]
        );
        let turn = turn.unwrap();
        assert_eq!(turn.stop_reason, StopReason::ToolUse);
        assert_eq!(
            turn.content[0],
            AssistantContent::Reasoning(Reasoning {
                summary: "Need the structure database.\n\nSearch TiO2.".into()
            })
        );
        assert_eq!(
            turn.tool_calls().next().unwrap().arguments,
            Some(json!({ "formula": "TiO2" }))
        );
        assert_eq!(
            turn.usage,
            Usage {
                input_tokens: 300,
                output_tokens: 120,
                cache_read_tokens: 2048,
                cache_write_tokens: 0
            }
        );
        assert_eq!(turn.vendor_stop_reason.as_deref(), Some("completed"));
    }

    #[test]
    fn decodes_refusal_and_incomplete() {
        let turn = decode_fixture("refusal").1.unwrap();
        assert_eq!(
            turn.stop_reason,
            StopReason::Refusal(Refusal {
                category: None,
                explanation: Some("I can't help with that.".into())
            })
        );
        let (events, turn) = decode_fixture("incomplete");
        assert_eq!(events, [StreamEvent::TextDelta("The cell".into())]);
        let turn = turn.unwrap();
        assert_eq!(turn.stop_reason, StopReason::MaxTokens);
        assert_eq!(
            turn.vendor_stop_reason.as_deref(),
            Some("incomplete: max_output_tokens")
        );
    }

    #[test]
    fn failures_and_error_events() {
        let (events, turn) = decode_fixture("failed");
        assert_eq!(events, [StreamEvent::TextDelta("Partial".into())]);
        let error = turn.unwrap_err();
        assert_eq!(
            error.to_string(),
            "stream error (server_error): The model failed to generate a response."
        );
        assert!(error.is_retryable());

        let (events, turn) = decode_fixture("error");
        assert!(events.is_empty());
        let error = turn.unwrap_err();
        assert_eq!(
            error.to_string(),
            "stream error (rate_limit_exceeded): Rate limit reached."
        );
        assert!(error.is_retryable());
    }

    #[test]
    fn a_stream_without_completion_is_incomplete() {
        let sse = "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"m\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hi\"}\n\n";
        let (events, turn) = decode(Decoder::new(origin("gpt-x")), sse);
        assert_eq!(events, [StreamEvent::TextDelta("Hi".into())]);
        assert_eq!(turn, Err(LlmError::Incomplete));
    }
}
