//! What each provider and model supports, and the built-in vendor profiles
//! (design §6.2.7).
//!
//! The backends read these flags to decide what to send; the assistant reads
//! them to adjust its behavior. Anything not known for certain is declared
//! unsupported. Profiles marked `reviewed: false` carry conservative guesses
//! that still need checking against the vendor's current documentation.

use serde::{Deserialize, Serialize};

use crate::Backend;

/// Feature flags and limits of one model at one endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Capabilities {
    /// Reasoning from earlier turns is sent back (Anthropic thinking blocks,
    /// Responses encrypted reasoning). Off: reasoning is dropped on replay.
    pub reasoning_replay: bool,
    /// Readable reasoning summaries can be requested for the UI.
    pub reasoning_summary: bool,
    /// Explicit cache breakpoints (Anthropic `cache_control`). Off: the
    /// request keeps a stable prefix and relies on automatic prefix caching.
    pub explicit_cache: bool,
    /// Tool definitions are sent with `strict: true`. Off: local argument
    /// validation is the only guarantee.
    pub strict_tools: bool,
    /// The model may call several tools in one turn.
    pub parallel_tool_calls: bool,
    /// Images in user messages and tool results.
    pub image_input: bool,
    /// Operator instructions can sit in the middle of the conversation. Off:
    /// they are appended to the system prompt, which breaks the cache.
    pub mid_conversation_system: bool,
    /// Server-side conversation summaries (Anthropic on-demand compaction).
    pub server_compaction: bool,
    /// The effort or reasoning-effort parameter is accepted.
    pub effort: bool,
    pub context_window: u32,
    pub max_output_tokens: u32,
}

impl Capabilities {
    /// For endpoints nothing is known about: every optional feature off.
    pub const CONSERVATIVE: Self = Self {
        reasoning_replay: false,
        reasoning_summary: false,
        explicit_cache: false,
        strict_tools: false,
        parallel_tool_calls: false,
        image_input: false,
        mid_conversation_system: false,
        server_compaction: false,
        effort: false,
        context_window: 32_768,
        max_output_tokens: 8_192,
    };

    /// Capabilities with overrides from the user's settings applied.
    pub fn with_overrides(mut self, overrides: &CapabilityOverrides) -> Self {
        let flags = [
            (&mut self.reasoning_replay, overrides.reasoning_replay),
            (&mut self.reasoning_summary, overrides.reasoning_summary),
            (&mut self.explicit_cache, overrides.explicit_cache),
            (&mut self.strict_tools, overrides.strict_tools),
            (&mut self.parallel_tool_calls, overrides.parallel_tool_calls),
            (&mut self.image_input, overrides.image_input),
            (
                &mut self.mid_conversation_system,
                overrides.mid_conversation_system,
            ),
            (&mut self.server_compaction, overrides.server_compaction),
            (&mut self.effort, overrides.effort),
        ];
        for (flag, value) in flags {
            if let Some(value) = value {
                *flag = value;
            }
        }
        if let Some(value) = overrides.context_window {
            self.context_window = value;
        }
        if let Some(value) = overrides.max_output_tokens {
            self.max_output_tokens = value;
        }
        self
    }
}

/// Per-capability settings that replace the profile's values. `None` keeps
/// the profile value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityOverrides {
    pub reasoning_replay: Option<bool>,
    pub reasoning_summary: Option<bool>,
    pub explicit_cache: Option<bool>,
    pub strict_tools: Option<bool>,
    pub parallel_tool_calls: Option<bool>,
    pub image_input: Option<bool>,
    pub mid_conversation_system: Option<bool>,
    pub server_compaction: Option<bool>,
    pub effort: Option<bool>,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

/// How the API key is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    /// No key, as for local servers.
    None,
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// `x-api-key: <key>` (Anthropic).
    XApiKey,
    /// `api-key: <key>` (Azure OpenAI).
    ApiKeyHeader,
}

/// Name of the output limit field in Chat Completions requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaxTokensField {
    /// `max_completion_tokens`, which OpenAI requires for reasoning models.
    MaxCompletionTokens,
    /// `max_tokens`, which compatible servers understand.
    MaxTokens,
}

/// Wire details that differ between services speaking the same protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Dialect {
    /// Chat Completions: field for the output limit.
    pub max_tokens_field: MaxTokensField,
    /// Chat Completions: role for system prompts, `system` or `developer`.
    pub system_role: &'static str,
    /// Chat Completions: send `stream_options.include_usage`.
    pub stream_usage: bool,
    /// Chat Completions: name of the non-standard reasoning field to send
    /// back when [`Capabilities::reasoning_replay`] is on. Incoming reasoning
    /// is read from both `reasoning_content` and `reasoning` regardless.
    pub reasoning_field: &'static str,
    /// Anthropic: `eager_input_streaming` on tools. Only for the official
    /// API, since proxies may reject the field.
    pub eager_input_streaming: bool,
}

impl Dialect {
    const COMPATIBLE: Self = Self {
        max_tokens_field: MaxTokensField::MaxTokens,
        system_role: "system",
        stream_usage: false,
        reasoning_field: "reasoning_content",
        eager_input_streaming: false,
    };

    const OPENAI: Self = Self {
        max_tokens_field: MaxTokensField::MaxCompletionTokens,
        system_role: "developer",
        stream_usage: true,
        ..Self::COMPATIBLE
    };

    const ANTHROPIC: Self = Self {
        eager_input_streaming: true,
        ..Self::COMPATIBLE
    };
}

/// Capabilities of one known model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelProfile {
    pub model: &'static str,
    pub capabilities: Capabilities,
}

/// A known service: where it is, how to talk to it, what it supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EndpointProfile {
    /// Stable identifier for settings, such as `anthropic` or `deepseek`.
    pub id: &'static str,
    pub name: &'static str,
    pub backend: Backend,
    /// Empty when every user has their own (Azure).
    pub base_url: &'static str,
    pub auth: Auth,
    pub dialect: Dialect,
    /// For models without their own entry in `models`.
    pub default_capabilities: Capabilities,
    pub models: &'static [ModelProfile],
    /// `false` while the values are conservative guesses awaiting review.
    pub reviewed: bool,
}

impl EndpointProfile {
    pub fn capabilities(&self, model: &str) -> Capabilities {
        self.models
            .iter()
            .find(|profile| profile.model == model)
            .map_or(self.default_capabilities, |profile| profile.capabilities)
    }

    /// A user-added endpoint: the most conservative settings (design §6.2.7).
    pub fn custom(backend: Backend) -> Self {
        Self {
            id: "custom",
            name: "Custom endpoint",
            backend,
            base_url: "",
            auth: Auth::Bearer,
            dialect: Dialect::COMPATIBLE,
            default_capabilities: Capabilities::CONSERVATIVE,
            models: &[],
            reviewed: false,
        }
    }
}

/// Claude 5.5 models: verified against the Claude API docs on 2026-10-22.
const CLAUDE_5_5: Capabilities = Capabilities {
    reasoning_replay: true,
    reasoning_summary: true,
    explicit_cache: true,
    strict_tools: true,
    parallel_tool_calls: true,
    image_input: true,
    mid_conversation_system: true,
    server_compaction: true,
    effort: true,
    context_window: 1_000_000,
    max_output_tokens: 128_000,
};

/// Other Claude models: thinking blocks still go back, and caching and images
/// work on every current model. Strict tools are missing on some (Opus 4.6
/// and 4.7, Sonnet 4.6), and the newer features are off.
const CLAUDE_OTHER: Capabilities = Capabilities {
    reasoning_summary: false,
    strict_tools: false,
    mid_conversation_system: false,
    server_compaction: false,
    effort: false,
    context_window: 200_000,
    max_output_tokens: 32_000,
    ..CLAUDE_5_5
};

const CLAUDE_MODELS: &[ModelProfile] = &[
    ModelProfile {
        model: "claude-opus-5-5",
        capabilities: CLAUDE_5_5,
    },
    ModelProfile {
        model: "claude-sonnet-5-5",
        capabilities: CLAUDE_5_5,
    },
    ModelProfile {
        model: "claude-haiku-5-5",
        capabilities: CLAUDE_5_5,
    },
];

/// OpenAI's own models. Model IDs are configured by the user, so this
/// assumes a current reasoning model; limits are deliberately low until the
/// user sets the model's real ones.
const OPENAI_CHAT: Capabilities = Capabilities {
    reasoning_replay: false,
    reasoning_summary: false,
    explicit_cache: false,
    strict_tools: true,
    parallel_tool_calls: true,
    image_input: true,
    mid_conversation_system: true,
    server_compaction: false,
    effort: true,
    context_window: 128_000,
    max_output_tokens: 32_000,
};

const OPENAI_RESPONSES: Capabilities = Capabilities {
    reasoning_replay: true,
    reasoning_summary: true,
    ..OPENAI_CHAT
};

/// Local servers: small defaults that fit typical local models.
const LOCAL: Capabilities = Capabilities {
    context_window: 8_192,
    max_output_tokens: 4_096,
    ..Capabilities::CONSERVATIVE
};

const fn compatible(
    id: &'static str,
    name: &'static str,
    base_url: &'static str,
    auth: Auth,
    default_capabilities: Capabilities,
) -> EndpointProfile {
    EndpointProfile {
        id,
        name,
        backend: Backend::OpenAiChat,
        base_url,
        auth,
        dialect: Dialect::COMPATIBLE,
        default_capabilities,
        models: &[],
        reviewed: false,
    }
}

const PROFILES: &[EndpointProfile] = &[
    EndpointProfile {
        id: "anthropic",
        name: "Anthropic",
        backend: Backend::Anthropic,
        base_url: "https://api.anthropic.com",
        auth: Auth::XApiKey,
        dialect: Dialect::ANTHROPIC,
        default_capabilities: CLAUDE_OTHER,
        models: CLAUDE_MODELS,
        reviewed: true,
    },
    EndpointProfile {
        id: "openai",
        name: "OpenAI (Responses)",
        backend: Backend::OpenAiResponses,
        base_url: "https://api.openai.com/v1",
        auth: Auth::Bearer,
        dialect: Dialect::OPENAI,
        default_capabilities: OPENAI_RESPONSES,
        models: &[],
        reviewed: true,
    },
    EndpointProfile {
        id: "openai-chat",
        name: "OpenAI (Chat Completions)",
        backend: Backend::OpenAiChat,
        base_url: "https://api.openai.com/v1",
        auth: Auth::Bearer,
        dialect: Dialect::OPENAI,
        default_capabilities: OPENAI_CHAT,
        models: &[],
        reviewed: true,
    },
    EndpointProfile {
        id: "azure-openai",
        name: "Azure OpenAI (Responses)",
        backend: Backend::OpenAiResponses,
        // https://<resource>.openai.azure.com/openai/v1, per user.
        base_url: "",
        auth: Auth::ApiKeyHeader,
        dialect: Dialect::OPENAI,
        default_capabilities: OPENAI_RESPONSES,
        models: &[],
        reviewed: false,
    },
    // Compatible services: base URLs from each vendor's docs; capabilities
    // conservative until reviewed.
    compatible(
        "deepseek",
        "DeepSeek",
        "https://api.deepseek.com",
        Auth::Bearer,
        Capabilities::CONSERVATIVE,
    ),
    compatible(
        "dashscope",
        "通义千问（DashScope 兼容模式）",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        Auth::Bearer,
        Capabilities::CONSERVATIVE,
    ),
    compatible(
        "moonshot",
        "Kimi（Moonshot）",
        "https://api.moonshot.cn/v1",
        Auth::Bearer,
        Capabilities::CONSERVATIVE,
    ),
    compatible(
        "zhipu",
        "智谱 GLM",
        "https://open.bigmodel.cn/api/paas/v4",
        Auth::Bearer,
        Capabilities::CONSERVATIVE,
    ),
    compatible(
        "siliconflow",
        "硅基流动",
        "https://api.siliconflow.cn/v1",
        Auth::Bearer,
        Capabilities::CONSERVATIVE,
    ),
    compatible(
        "openrouter",
        "OpenRouter",
        "https://openrouter.ai/api/v1",
        Auth::Bearer,
        Capabilities::CONSERVATIVE,
    ),
    compatible(
        "ollama",
        "Ollama",
        "http://localhost:11434/v1",
        Auth::None,
        LOCAL,
    ),
    compatible(
        "vllm",
        "vLLM",
        "http://localhost:8000/v1",
        Auth::None,
        LOCAL,
    ),
    compatible(
        "lm-studio",
        "LM Studio",
        "http://localhost:1234/v1",
        Auth::None,
        LOCAL,
    ),
];

/// All built-in profiles.
pub fn endpoint_profiles() -> &'static [EndpointProfile] {
    PROFILES
}

pub fn endpoint_profile(id: &str) -> Option<&'static EndpointProfile> {
    PROFILES.iter().find(|profile| profile.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_ids_are_unique_and_found() {
        for profile in endpoint_profiles() {
            assert_eq!(endpoint_profile(profile.id), Some(profile));
        }
    }

    #[test]
    fn claude_models_and_fallback() {
        let anthropic = endpoint_profile("anthropic").unwrap();
        for model in ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"] {
            let caps = anthropic.capabilities(model);
            assert!(caps.reasoning_replay && caps.mid_conversation_system && caps.effort);
            assert_eq!(caps.max_output_tokens, 128_000);
        }
        let other = anthropic.capabilities("claude-opus-4-8");
        assert!(other.reasoning_replay);
        assert!(!other.mid_conversation_system && !other.effort);
    }

    #[test]
    fn compatible_services_are_conservative_and_unreviewed() {
        for profile in endpoint_profiles()
            .iter()
            .filter(|profile| profile.backend == Backend::OpenAiChat && profile.id != "openai-chat")
        {
            let caps = profile.default_capabilities;
            assert!(!profile.reviewed, "{}", profile.id);
            assert!(!caps.reasoning_replay && !caps.strict_tools && !caps.image_input);
            assert!(!caps.mid_conversation_system && !caps.parallel_tool_calls);
            assert_eq!(profile.dialect.max_tokens_field, MaxTokensField::MaxTokens);
        }
    }

    #[test]
    fn custom_endpoints_get_the_most_conservative_settings() {
        let custom = EndpointProfile::custom(Backend::OpenAiResponses);
        assert_eq!(custom.capabilities("anything"), Capabilities::CONSERVATIVE);
        assert!(!custom.dialect.eager_input_streaming);
    }

    #[test]
    fn overrides_replace_only_what_they_set() {
        let overrides = CapabilityOverrides {
            image_input: Some(true),
            context_window: Some(131_072),
            ..Default::default()
        };
        let caps = Capabilities::CONSERVATIVE.with_overrides(&overrides);
        assert!(caps.image_input);
        assert_eq!(caps.context_window, 131_072);
        assert_eq!(
            Capabilities {
                image_input: false,
                context_window: 32_768,
                ..caps
            },
            Capabilities::CONSERVATIVE
        );
    }
}
