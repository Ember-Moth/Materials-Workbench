//! Sends one user message to a model and prints the stream as it arrives.
//!
//! Configured through environment variables, so the key never appears on the
//! command line or in the code:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `MW_LLM_PROVIDER` | A profile id (`anthropic`, `openai`, `openai-chat`, `deepseek`, `ollama`…) or, for an endpoint of your own with the most conservative capabilities, `custom:<backend>` (`custom:openai_chat`, `custom:openai_responses`, `custom:anthropic`) together with `MW_LLM_BASE_URL`. Default `anthropic`. |
//! | `MW_LLM_BASE_URL` | Overrides the profile's base URL. |
//! | `MW_LLM_MODEL` | Model name. Default `claude-opus-5-5` for `anthropic`; required otherwise. |
//! | `MW_LLM_API_KEY` | API key; leave unset for local servers. |
//! | `MW_LLM_EFFORT` | `low`, `medium`, `high`, `xhigh` or `max`. |
//! | `MW_LLM_TOOLS` | `1` offers a demo tool, to see a tool call stream. |
//!
//! The message is the command-line arguments joined by spaces.
//!
//! ```text
//! MW_LLM_API_KEY=… cargo run -p mw-llm --example chat -- "What is a Wyckoff position?"
//! ```

use std::env;
use std::io::Write;

use futures::StreamExt;
use mw_llm::{
    ApiKey, Backend, ChatRequest, Effort, EndpointProfile, Message, ProviderConfig, RequestOptions,
    Runtime, StreamEvent, ToolDefinition, endpoint_profile, schema,
};
use serde_json::json;

fn var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn demo_tool() -> ToolDefinition {
    ToolDefinition::new(
        "structure_neighbors",
        "List the atoms within a cutoff distance of a site in the open crystal structure. \
         Call this when the user asks about coordination, bond lengths or neighbors.",
        json!({
            "type": "object",
            "properties": {
                "site": { "type": "string", "description": "Site label, such as Ti1." },
                "cutoff_angstrom": {
                    "type": ["number", "null"],
                    "description": "Cutoff in Å, 0.5 to 6; null for the default of 3."
                }
            },
            "required": ["site", "cutoff_angstrom"],
            "additionalProperties": false
        }),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let provider_id = var("MW_LLM_PROVIDER").unwrap_or_else(|| "anthropic".into());
    let base_url = var("MW_LLM_BASE_URL");
    let custom = provider_id
        .strip_prefix("custom:")
        .and_then(Backend::from_id);
    let profile: EndpointProfile = match (custom, endpoint_profile(&provider_id)) {
        (Some(backend), _) => EndpointProfile::custom(backend),
        (None, Some(profile)) => *profile,
        (None, None) => {
            return Err(format!(
                "unknown provider {provider_id:?}; use a profile id or custom:<backend>"
            )
            .into());
        }
    };
    let model = match var("MW_LLM_MODEL") {
        Some(model) => model,
        None if profile.id == "anthropic" => "claude-opus-5-5".into(),
        None => return Err("set MW_LLM_MODEL".into()),
    };

    let mut config = ProviderConfig::new(&profile, var("MW_LLM_API_KEY").map(ApiKey::new));
    if let Some(url) = base_url {
        config.base_url = url;
    }
    let runtime = Runtime::new()?;
    let provider = mw_llm::connect(config, &runtime)?;

    let mut options = RequestOptions::new(&model);
    if let Some(effort) = var("MW_LLM_EFFORT") {
        options.effort = Some(Effort::parse(&effort).ok_or("unknown MW_LLM_EFFORT")?);
    }
    let mut request = ChatRequest::new(options);
    request.system = "You are the assistant in Materials Workbench, a desktop application for \
                      crystallography. Be concise."
        .into();
    let tools = var("MW_LLM_TOOLS").as_deref() == Some("1");
    if tools {
        request.tools.push(demo_tool());
    }
    let words: Vec<String> = env::args().skip(1).collect();
    let prompt = if words.is_empty() {
        if tools {
            "What are the Ti–O bond lengths around Ti1 in this rutile structure?".to_owned()
        } else {
            "In two sentences: what is a Wyckoff position?".to_owned()
        }
    } else {
        words.join(" ")
    };
    request.messages.push(Message::user_text(prompt));

    let caps = provider.capabilities(&model);
    eprintln!(
        "→ {} at {} ({model}); strict tools {}, reasoning replay {}, images {}",
        provider.backend().id(),
        provider.endpoint(),
        caps.strict_tools,
        caps.reasoning_replay,
        caps.image_input
    );

    let mut stream = provider.stream(request);
    let mut reasoning = false;
    futures::executor::block_on(async {
        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::ReasoningDelta(text)) => {
                    if !reasoning {
                        eprint!("\x1b[2m[reasoning] ");
                        reasoning = true;
                    }
                    eprint!("{text}");
                }
                Ok(event) => {
                    if reasoning {
                        eprintln!("\x1b[0m");
                        reasoning = false;
                    }
                    match event {
                        StreamEvent::TextDelta(text) => print!("{text}"),
                        StreamEvent::ToolCallStart { id, name } => {
                            print!("\n[tool call {name} {id}] ");
                        }
                        StreamEvent::ToolCallDelta { arguments, .. } => print!("{arguments}"),
                        StreamEvent::Done(turn) => {
                            println!();
                            eprintln!(
                                "← stop {:?} ({}), usage {:?}",
                                turn.stop_reason,
                                turn.vendor_stop_reason.as_deref().unwrap_or("-"),
                                turn.usage
                            );
                            for call in turn.tool_calls() {
                                let checked = if !turn.may_run_tools() {
                                    "not run: the turn was cut off or refused".to_owned()
                                } else {
                                    match schema::validate_arguments(
                                        &demo_tool().input_schema,
                                        call,
                                    ) {
                                        Ok(arguments) => format!("valid: {arguments}"),
                                        Err(error) => format!("rejected: {error}"),
                                    }
                                };
                                eprintln!("  tool call {} {}", call.name, checked);
                            }
                        }
                        _ => {}
                    }
                }
                Err(error) => eprintln!("\n✗ {error}"),
            }
            std::io::stdout().flush().ok();
        }
    });
    Ok(())
}
