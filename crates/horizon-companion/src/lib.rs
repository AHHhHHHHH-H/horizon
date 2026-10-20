//! The companion's brain, without the game (docs/companions.md): what to download and from where, what
//! the PC can run, the llama-server supervisor, the OpenAI-compatible streaming client, the prompts,
//! and the reflexes that answer common commands without a model.
//!
//! Nothing here blocks a caller's frame: downloads, the runtime and the chat client run on their own
//! threads and report over channels the game polls once per tick.

pub mod catalogue;
pub mod client;
pub mod download;
pub mod hardware;
pub mod phrases;
pub mod plan;
pub mod prompt;
pub mod reflex;
pub mod reply;
pub mod runtime;
pub mod settings;
