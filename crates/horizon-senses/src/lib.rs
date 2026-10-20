//! The companion's senses (docs/companions.md, slice 6), without the game:
//! - [`catalogue`]: the pinned audio.cpp builds and the speech to text, text to speech and classifier
//!   models, with what each needs and how fast it was measured;
//! - [`sidecar`]: a low-priority server process on 127.0.0.1 that dies with the game;
//! - [`voice`]: audio.cpp's server, streaming text to speech and push-to-talk speech to text;
//! - [`classifier`]: the reflexes' intent classifier (an embedding kNN over a multilingual bank);
//! - [`dsp`]: PCM and WAV, 16 kHz resampling for the microphone, and the radio effect.
//!
//! Everything blocking runs on worker threads and talks over std channels; nothing here starts
//! unless the player turned the companion's voice or classifier on.
pub mod catalogue;
pub mod classifier;
pub mod dsp;
pub mod sidecar;
pub mod voice;
