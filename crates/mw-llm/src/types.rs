//! The conversation in the project's own terms (design §6.2.2).
//!
//! Every backend converts these types to and from its wire format. Assistant
//! turns also keep the vendor's raw JSON and where they came from, so a turn can
//! be sent back to the same model unchanged ([`AssistantTurn::origin`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The wire protocol a provider speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// Anthropic Messages API (`POST /v1/messages`).
    Anthropic,
    /// OpenAI Chat Completions (`POST /chat/completions`), also spoken by
    /// compatible services and local servers.
    OpenAiChat,
    /// OpenAI Responses (`POST /responses`) in stateless mode.
    OpenAiResponses,
}

impl Backend {
    /// Stable identifier used in configuration: `anthropic`, `openai_chat`,
    /// `openai_responses`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAiChat => "openai_chat",
            Self::OpenAiResponses => "openai_responses",
        }
    }

    /// Inverse of [`Backend::id`].
    pub fn from_id(id: &str) -> Option<Self> {
        [Self::Anthropic, Self::OpenAiChat, Self::OpenAiResponses]
            .into_iter()
            .find(|backend| backend.id() == id)
    }
}

/// Where an assistant turn was produced.
///
/// A turn's raw vendor data is replayed only to the exact same origin; any
/// other target gets the turn rebuilt from its text, tool calls and tool
/// results, without reasoning (design §6.2.2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Origin {
    pub backend: Backend,
    /// The base URL the request went to, so that two services that happen to
    /// use the same model name are not mistaken for each other.
    pub endpoint: String,
    /// The model named in the request.
    pub model: String,
}

/// One entry of the conversation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Message {
    /// The user's input, and the results of the tool calls of the previous
    /// assistant turn.
    User(Vec<UserContent>),
    /// A turn produced by a model, as [`crate::StreamEvent::Done`] delivers it.
    Assistant(Box<AssistantTurn>),
    /// An operator instruction added partway through the conversation. It
    /// takes effect from the next user message on; backends that cannot carry
    /// it in place append it to the system prompt instead (design §6.2.7).
    System(String),
}

impl Message {
    /// A user message with a single text block.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::User(vec![UserContent::Text(text.into())])
    }
}

/// Content the user side of the conversation can send.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserContent {
    Text(String),
    /// For example a viewport snapshot.
    Image(Image),
    ToolResult(ToolResult),
}

/// A base64-encoded image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    pub media_type: String,
    /// Base64 without line breaks or a `data:` prefix.
    pub data: String,
}

impl Image {
    pub fn new(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            media_type: media_type.into(),
            data: data.into(),
        }
    }

    pub(crate) fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.media_type, self.data)
    }
}

/// The outcome of one tool call, sent back in the next user message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// [`ToolCall::id`] of the call this answers.
    pub call_id: String,
    pub content: Vec<ToolResultContent>,
    /// The call failed, or its arguments were rejected before it ran. The
    /// content then explains why, so the model can correct itself.
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            content: vec![ToolResultContent::Text(text.into())],
            is_error: false,
        }
    }

    pub fn error(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            is_error: true,
            ..Self::text(call_id, text)
        }
    }

    /// The text parts joined by blank lines.
    pub fn joined_text(&self) -> String {
        let parts: Vec<&str> = self
            .content
            .iter()
            .filter_map(|part| match part {
                ToolResultContent::Text(text) => Some(text.as_str()),
                ToolResultContent::Image(_) => None,
            })
            .collect();
        parts.join("\n\n")
    }

    pub(crate) fn images(&self) -> impl Iterator<Item = &Image> {
        self.content.iter().filter_map(|part| match part {
            ToolResultContent::Image(image) => Some(image),
            ToolResultContent::Text(_) => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultContent {
    Text(String),
    Image(Image),
}

/// Content of an assistant turn, in the order the model produced it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AssistantContent {
    Text(String),
    ToolCall(ToolCall),
    /// Reasoning is opaque to the caller: only a readable summary is exposed.
    /// The data needed to send it back (signatures, encrypted content) stays
    /// in [`AssistantTurn::raw`].
    Reasoning(Reasoning),
    /// A server-side summary that replaces the conversation before it
    /// (Anthropic on-demand compaction, design §6.2.7).
    Compaction(Compaction),
}

/// A request from the model to run a tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The arguments exactly as the model produced them.
    pub arguments_raw: String,
    /// [`ToolCall::arguments_raw`] parsed as JSON, or `None` if it is not
    /// valid JSON. Not yet checked against the tool's schema: use
    /// [`crate::schema::validate_arguments`] before running the tool.
    pub arguments: Option<Value>,
}

impl ToolCall {
    /// Parses `arguments_raw`; an empty string counts as `{}`, which is what
    /// providers send for tools without parameters.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments_raw: impl Into<String>,
    ) -> Self {
        let arguments_raw = arguments_raw.into();
        let arguments = if arguments_raw.trim().is_empty() {
            Some(Value::Object(Default::default()))
        } else {
            serde_json::from_str(&arguments_raw).ok()
        };
        Self {
            id: id.into(),
            name: name.into(),
            arguments_raw,
            arguments,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reasoning {
    /// Readable summary, empty when the provider returned none.
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compaction {
    pub summary: String,
}

/// One complete response from a model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssistantTurn {
    pub origin: Origin,
    pub content: Vec<AssistantContent>,
    pub stop_reason: StopReason,
    /// The vendor's own value (`end_turn`, `length`, `max_output_tokens`…),
    /// for logs and diagnostics.
    pub vendor_stop_reason: Option<String>,
    pub usage: Usage,
    /// The turn as the vendor returned it: Anthropic's `content` array, the
    /// assembled Chat Completions `message`, or the Responses `output` items.
    /// `Null` when unknown, in which case the turn is always rebuilt from
    /// [`AssistantTurn::content`].
    pub raw: Value,
}

impl AssistantTurn {
    /// A turn without vendor data, for scripted providers in tests.
    pub fn new(origin: Origin, content: Vec<AssistantContent>, stop_reason: StopReason) -> Self {
        Self {
            origin,
            content,
            stop_reason,
            vendor_stop_reason: None,
            usage: Usage::default(),
            raw: Value::Null,
        }
    }

    /// All text blocks, concatenated.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.content.iter().filter_map(|block| match block {
            AssistantContent::ToolCall(call) => Some(call),
            _ => None,
        })
    }

    /// Whether this turn's tool calls may run at all. A turn cut off by the
    /// output limit or ended by a refusal can hold incomplete calls, so none
    /// of them run (design §6.2.6).
    pub fn may_run_tools(&self) -> bool {
        !matches!(
            self.stop_reason,
            StopReason::MaxTokens | StopReason::Refusal(_)
        )
    }

    /// Whether the raw vendor data can be sent to `target` unchanged.
    pub fn replays_raw_to(&self, target: &Origin) -> bool {
        self.origin == *target && !self.raw.is_null()
    }
}

/// Why the model stopped, unified across vendors (design §6.2.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StopReason {
    /// The model finished its answer.
    EndTurn,
    /// The model wants tool results before it continues.
    ToolUse,
    /// The output limit, or the context window, ran out.
    MaxTokens,
    /// The model or a safety filter declined. Partial output is incomplete.
    Refusal(Refusal),
    /// The response is a compaction summary instead of a reply.
    Compacted,
    /// Anything else, with the vendor's value (for example `pause_turn`).
    Other(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// Policy category when the vendor names one (Anthropic `cyber`, `bio`…;
    /// `content_filter` for OpenAI filters).
    pub category: Option<String>,
    /// Human-readable explanation; show it, don't parse it.
    pub explanation: Option<String>,
}

/// Token counts of one turn. The four fields are disjoint: total input is
/// `input_tokens + cache_read_tokens + cache_write_tokens`, and each part is
/// billed at its own rate. Output includes reasoning tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens billed at the normal rate.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
    }
}

/// A tool the model may call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// `^[a-zA-Z0-9_-]{1,64}$`, the rule both vendors accept.
    pub name: String,
    pub description: String,
    /// A JSON Schema in the strict subset of [`crate::schema`].
    pub input_schema: Value,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

/// How much the model should think. Each backend sends the level only when
/// the model declares support for it ([`crate::Capabilities::effort`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl Effort {
    /// The value both vendors use on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// Inverse of [`Effort::as_str`].
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Low, Self::Medium, Self::High, Self::XHigh, Self::Max]
            .into_iter()
            .find(|effort| effort.as_str() == value)
    }
}

/// Per-request settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RequestOptions {
    pub model: String,
    /// Upper bound on output, reasoning included. `None` uses the model's
    /// [`crate::Capabilities::max_output_tokens`]; larger values are clamped
    /// to it.
    pub max_output_tokens: Option<u32>,
    /// `None` leaves the model's default, which differs between models
    /// (Claude Opus 5.5 defaults to `medium`, Claude Sonnet 5.5 to `high`).
    pub effort: Option<Effort>,
    /// Ask for readable reasoning summaries to stream to the UI, where the
    /// model supports it.
    pub reasoning_summary: bool,
    /// Ask for a summary of the conversation instead of a reply (Anthropic
    /// on-demand compaction). Needs [`crate::Capabilities::server_compaction`].
    pub compaction: Option<CompactionRequest>,
}

impl RequestOptions {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            max_output_tokens: None,
            effort: None,
            reasoning_summary: true,
            compaction: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionRequest {
    /// Replaces the default summarization prompt.
    pub instructions: Option<String>,
}

/// Everything one model call needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The stable system prompt. Keep it identical for a whole conversation;
    /// later instructions go in [`Message::System`] (design §6.2.3).
    pub system: String,
    pub messages: Vec<Message>,
    /// Keep the set and its order fixed for a conversation; it is the start
    /// of the cached prefix.
    pub tools: Vec<ToolDefinition>,
    pub options: RequestOptions,
}

impl ChatRequest {
    pub fn new(options: RequestOptions) -> Self {
        Self {
            system: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            options,
        }
    }
}
