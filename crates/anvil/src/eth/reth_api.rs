//! Reth provider integration for read-side RPC delegation.
//!
//! Wraps `AnvilProvider` and delegates selected `eth_*` methods through reth's
//! storage traits as a proof of concept.

use alloy_primitives::{Address, U256};
use anvil_reth_bridge::{AnvilProvider, BackendView};
use reth_chainspec::ChainSpec;
use reth_storage_api::{BlockNumReader, StateProviderFactory};
use std::sync::Arc;

/// Read-side RPC adapter backed by `AnvilProvider`.
///
/// Delegates selected `eth_*` calls through reth's storage traits.
pub struct RethReadAdapter<B: BackendView> {
    provider: AnvilProvider<B>,
}

impl<B: BackendView> RethReadAdapter<B> {
    pub fn new(backend: Arc<B>, chain_spec: Arc<ChainSpec>) -> Self {
        Self { provider: AnvilProvider::new(backend, chain_spec) }
    }

    /// `eth_blockNumber` — returns the latest block number.
    pub fn block_number(&self) -> Result<U256, String> {
        self.provider
            .best_block_number()
            .map(U256::from)
            .map_err(|e| e.to_string())
    }

    /// `eth_getBalance` — returns the balance of an account at the given block.
    pub fn get_balance(
        &self,
        address: Address,
        block: Option<alloy_eips::BlockId>,
    ) -> Result<U256, String> {
        use reth_storage_api::{AccountReader, BlockIdReader};

        let state = match block {
            Some(id) => {
                let hash = self
                    .provider
                    .block_hash_for_id(id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "block not found".to_string())?;
                self.provider.history_by_block_hash(hash).map_err(|e| e.to_string())?
            }
            None => self.provider.latest().map_err(|e| e.to_string())?,
        };

        match state.basic_account(&address) {
            Ok(Some(account)) => Ok(account.balance),
            Ok(None) => Ok(U256::ZERO),
            Err(e) => Err(e.to_string()),
        }
    }

    /// `eth_getTransactionCount` — returns the nonce of an account at the given block.
    pub fn get_transaction_count(
        &self,
        address: Address,
        block: Option<alloy_eips::BlockId>,
    ) -> Result<U256, String> {
        use reth_storage_api::{AccountReader, BlockIdReader};

        let state = match block {
            Some(id) => {
                let hash = self
                    .provider
                    .block_hash_for_id(id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "block not found".to_string())?;
                self.provider.history_by_block_hash(hash).map_err(|e| e.to_string())?
            }
            None => self.provider.latest().map_err(|e| e.to_string())?,
        };

        match state.basic_account(&address) {
            Ok(Some(account)) => Ok(U256::from(account.nonce)),
            Ok(None) => Ok(U256::ZERO),
            Err(e) => Err(e.to_string()),
        }
    }
}

impl<B: BackendView> Clone for RethReadAdapter<B> {
    fn clone(&self) -> Self {
        Self { provider: self.provider.clone() }
    }
}
