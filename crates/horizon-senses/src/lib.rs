//! The companion's senses (docs/companions.md, slice 6), without the game. What they run (audio.cpp
//! and the models, pinned) is listed in `horizon_companion::senses`, beside the brain's catalogue, so
//! the launcher can plan and download everything without this crate; this crate runs it:
//! - [`sidecar`]: a low-priority server process on 127.0.0.1 that dies with the game;
//! - [`voice`]: audio.cpp's server, streaming text to speech and push-to-talk speech to text;
//! - [`classifier`]: the reflexes' intent classifier (an embedding kNN over a multilingual bank);
//! - [`dsp`]: PCM and WAV, 16 kHz resampling for the microphone, and the radio effect.
//!
//! Everything blocking runs on worker threads and talks over std channels; nothing here starts
//! unless the player turned the companion's voice or classifier on.
pub mod classifier;
pub mod dsp;
pub mod sidecar;
pub mod voice;
