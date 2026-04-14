//! Anvil ↔ Reth RPC Bridge
//!
//! Adapts anvil's in-memory backend to reth's RPC provider traits,
//! enabling reuse of reth's generic `EthApi` implementation.

pub mod convert;
pub mod network;
pub mod provider;

pub use network::AnvilNetwork;
pub use provider::{AnvilProvider, AnvilStateProvider};
