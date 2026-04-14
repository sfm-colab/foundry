//! Trait abstracting read-only access to anvil's backend storage.
//!
//! This trait lets `anvil-reth-bridge` work without depending on the `anvil` crate,
//! breaking the cyclic dependency.

use alloy_consensus::TxType;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{B256, Log};
use anvil_core::eth::{block::Block as AnvilBlock, transaction::TransactionInfo};
use foundry_evm::backend::DatabaseError;
use foundry_primitives::FoundryReceiptEnvelope;
use revm::database::DatabaseRef;
use std::fmt;

/// Metadata for a mined transaction, extracted from anvil's storage.
#[derive(Clone, Debug)]
pub struct MinedTx<R> {
    /// Transaction metadata (hash, index, traces, etc.)
    pub info: TransactionInfo,
    /// The receipt envelope.
    pub receipt: R,
    /// Hash of the block containing this transaction.
    pub block_hash: B256,
    /// Number of the block containing this transaction.
    pub block_number: u64,
}

/// A receipt that the bridge can convert to reth's `Receipt` type.
pub trait ReceiptView: Clone + fmt::Debug + Send + Sync + 'static {
    /// Returns the Ethereum tx type, or `None` for non-standard types (Deposit, Tempo).
    fn tx_type(&self) -> Option<TxType>;
    /// Whether the transaction was successful.
    fn success(&self) -> bool;
    /// Cumulative gas used up to and including this transaction.
    fn cumulative_gas_used(&self) -> u64;
    /// Logs emitted by this transaction.
    fn logs(&self) -> Vec<Log>;
}

/// Read-only view of anvil's backend, providing access to blocks, transactions, and state.
///
/// This trait is implemented by anvil's `Backend<FoundryNetwork>` to feed data
/// to the reth provider adapters without creating a dependency cycle.
pub trait BackendView: fmt::Debug + Send + Sync + 'static {
    /// The state snapshot type, implementing revm's `DatabaseRef`.
    type State: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync + 'static;
    /// The receipt type stored in mined transactions.
    type Receipt: ReceiptView;

    /// Returns the hash of the current best block.
    fn best_hash(&self) -> B256;
    /// Returns the number of the current best block.
    fn best_number(&self) -> u64;

    /// Resolves a block number/tag to a block hash.
    fn block_hash(&self, id: BlockNumberOrTag) -> Option<B256>;
    /// Returns the hash for the given block number, if it exists.
    fn number_to_hash(&self, number: u64) -> Option<B256>;
    /// Returns `true` if a block with the given hash exists in storage.
    fn has_block(&self, hash: B256) -> bool;
    /// Returns the block for the given hash, if it exists.
    fn block_by_hash(&self, hash: B256) -> Option<AnvilBlock>;

    /// Returns a mined transaction by its hash, if it exists.
    fn mined_transaction_by_hash(&self, hash: B256) -> Option<MinedTx<Self::Receipt>>;

    /// Returns all transaction hashes in the block identified by hash.
    fn block_transaction_hashes(&self, block_hash: B256) -> Vec<B256>;

    /// Returns an owned, standalone snapshot of the latest state.
    fn latest_state(&self) -> Result<Self::State, DatabaseError>;

    /// Returns an owned, standalone snapshot for the given block hash.
    /// Returns `Ok(None)` if the state for that block is unavailable.
    fn state_by_block_hash(&self, hash: B256) -> Result<Option<Self::State>, DatabaseError>;
}

// =============================================================================
// ReceiptView for FoundryReceiptEnvelope
// =============================================================================

impl ReceiptView for FoundryReceiptEnvelope {
    fn tx_type(&self) -> Option<TxType> {
        match self {
            Self::Legacy(_) => Some(TxType::Legacy),
            Self::Eip2930(_) => Some(TxType::Eip2930),
            Self::Eip1559(_) => Some(TxType::Eip1559),
            Self::Eip4844(_) => Some(TxType::Eip4844),
            Self::Eip7702(_) => Some(TxType::Eip7702),
            Self::Deposit(_) | Self::Tempo(_) => None,
        }
    }

    fn success(&self) -> bool {
        match self {
            Self::Legacy(r)
            | Self::Eip2930(r)
            | Self::Eip1559(r)
            | Self::Eip4844(r)
            | Self::Eip7702(r)
            | Self::Tempo(r) => r.receipt.status.coerce_status(),
            Self::Deposit(r) => r.receipt.inner.status.coerce_status(),
        }
    }

    fn cumulative_gas_used(&self) -> u64 {
        match self {
            Self::Legacy(r)
            | Self::Eip2930(r)
            | Self::Eip1559(r)
            | Self::Eip4844(r)
            | Self::Eip7702(r)
            | Self::Tempo(r) => r.receipt.cumulative_gas_used,
            Self::Deposit(r) => r.receipt.inner.cumulative_gas_used,
        }
    }

    fn logs(&self) -> Vec<Log> {
        match self {
            Self::Legacy(r)
            | Self::Eip2930(r)
            | Self::Eip1559(r)
            | Self::Eip4844(r)
            | Self::Eip7702(r)
            | Self::Tempo(r) => r.receipt.logs.clone(),
            Self::Deposit(r) => r.receipt.inner.logs.clone(),
        }
    }
}
