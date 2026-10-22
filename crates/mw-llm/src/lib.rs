//! Model service clients for the built-in assistant, independent of any UI
//! (`docs/design/ai-integration.md` §6.2).
//!
//! The assistant works with the project's own conversation types
//! ([`ChatRequest`], [`Message`], [`AssistantTurn`]) and one interface,
//! [`ChatProvider`]. Three backends implement it: Anthropic Messages, OpenAI
//! Chat Completions (also for compatible services and local servers), and
//! OpenAI Responses in stateless mode.
//!
//! ```no_run
//! use futures::StreamExt;
//! use mw_llm::{ApiKey, ChatRequest, Message, ProviderConfig, RequestOptions, Runtime, StreamEvent};
//!
//! let runtime = Runtime::new()?;
//! let profile = mw_llm::endpoint_profile("anthropic").unwrap();
//! let provider = mw_llm::connect(ProviderConfig::new(profile, Some(ApiKey::new("…"))), &runtime)?;
//!
//! let mut request = ChatRequest::new(RequestOptions::new("claude-opus-5-5"));
//! request.messages.push(Message::user_text("What is a Wyckoff position?"));
//! let mut stream = provider.stream(request);
//! futures::executor::block_on(async {
//!     while let Some(event) = stream.next().await {
//!         match event {
//!             Ok(StreamEvent::TextDelta(text)) => print!("{text}"),
//!             Ok(StreamEvent::Done(turn)) => println!("\n{:?}", turn.stop_reason),
//!             Ok(_) => {}
//!             Err(error) => eprintln!("{error}"),
//!         }
//!     }
//! });
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! HTTP runs on a [`Runtime`] thread; the stream can be polled from any
//! executor, GPUI's included, and dropping it cancels the request.

mod anthropic;
pub mod capabilities;
mod driver;
mod error;
mod openai_chat;
mod openai_responses;
mod provider;
mod runtime;
pub mod schema;
mod sse;
mod types;

pub use capabilities::{
    Auth, Capabilities, CapabilityOverrides, EndpointProfile, endpoint_profile, endpoint_profiles,
};
pub use driver::RetryPolicy;
pub use error::{ApiError, LlmError};
pub use provider::{
    ApiKey, ChatProvider, ChatStream, ProviderConfig, StreamEvent, Timeouts, connect,
};
pub use runtime::Runtime;
pub use types::*;
