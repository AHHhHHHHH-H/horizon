//! The companion without the game (docs/companions.md): what to download and from where (the brain's
//! catalogue and the senses'), what the PC can run and the plan for it, setting it up, the
//! llama-server supervisor, the OpenAI-compatible streaming client, the prompts, and the reflexes that
//! answer common commands without a model. The launcher and the game both use it; the senses' sidecars
//! themselves are in `horizon-senses`.
//!
//! Nothing here blocks a caller's frame: downloads, the runtime and the chat client run on their own
//! threads and report over channels the game polls once per tick.

pub mod catalogue;
pub mod client;
pub mod download;
pub mod guard;
pub mod hardware;
pub mod partner;
pub mod phrases;
pub mod plan;
pub mod prompt;
pub mod reflex;
pub mod reply;
pub mod runtime;
pub mod senses;
pub mod settings;
pub mod setup;
