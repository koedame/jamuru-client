//! jamjam - Low-latency P2P audio communication for musicians
//!
//! This library provides the core functionality for real-time audio
//! streaming between musicians over a network.

pub mod audio;
pub mod config;
pub mod diagnostics;
pub mod environment;
pub mod identity_store;
pub mod network;
pub mod protocol;

pub use audio::{AudioConfig, AudioEngine, AudioPreset};
pub use diagnostics::{
    run_complete_diagnostics, AudioDiagnostics, CompleteDiagnosticsResult, CpuDiagnostics,
    DiagnosticGrade, NetworkDiagnostics, RecommendedPreset,
};
pub use network::Connection;
pub use protocol::Packet;
