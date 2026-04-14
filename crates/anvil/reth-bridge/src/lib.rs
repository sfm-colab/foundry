//! Anvil ↔ Reth RPC Bridge
//!
//! Adapts anvil's in-memory backend to reth's RPC provider traits,
//! enabling reuse of reth's generic `EthApi` implementation.

pub mod backend;
pub mod convert;
pub mod network;
pub mod provider;

pub use backend::{BackendView, MinedTx, ReceiptView};
pub use network::AnvilNetwork;
pub use provider::{AnvilProvider, AnvilStateProvider};
