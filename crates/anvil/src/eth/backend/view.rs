//! Implements [`BackendView`] for anvil's concrete `Backend<FoundryNetwork>`,
//! bridging the in-memory backend to the generic reth provider adapters.

use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use alloy_rpc_types::BlockId;
use anvil_reth_bridge::backend::{BackendView, MinedTx};
use foundry_evm::backend::DatabaseError;
use foundry_primitives::{FoundryNetwork, FoundryReceiptEnvelope};

use crate::eth::backend::{
    db::{MaybeFullDatabase, StateDb},
    mem::Backend,
};

// =============================================================================
// BackendView for Backend<FoundryNetwork>
// =============================================================================

impl BackendView for Backend<FoundryNetwork> {
    type State = StateDb;
    type Receipt = FoundryReceiptEnvelope;

    fn best_hash(&self) -> B256 {
        self.best_hash()
    }

    fn best_number(&self) -> u64 {
        self.best_number()
    }

    fn block_hash(&self, id: BlockNumberOrTag) -> Option<B256> {
        self.blockchain().storage.read().hash(id)
    }

    fn number_to_hash(&self, number: u64) -> Option<B256> {
        self.blockchain().storage.read().hashes.get(&number).copied()
    }

    fn has_block(&self, hash: B256) -> bool {
        self.blockchain().storage.read().blocks.contains_key(&hash)
    }

    fn block_by_hash(&self, hash: B256) -> Option<anvil_core::eth::block::Block> {
        self.blockchain().storage.read().blocks.get(&hash).cloned()
    }

    fn mined_transaction_by_hash(&self, hash: B256) -> Option<MinedTx<Self::Receipt>> {
        let storage = self.blockchain().storage.read();
        storage.transactions.get(&hash).map(|mined| MinedTx {
            info: mined.info.clone(),
            receipt: mined.receipt.clone(),
            block_hash: mined.block_hash,
            block_number: mined.block_number,
        })
    }

    fn block_transaction_hashes(&self, block_hash: B256) -> Vec<B256> {
        let storage = self.blockchain().storage.read();
        storage
            .blocks
            .get(&block_hash)
            .map(|block| {
                block
                    .body
                    .transactions
                    .iter()
                    .map(|tx| tx.hash())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn latest_state(&self) -> Result<Self::State, DatabaseError> {
        match self.get_db().try_read() {
            Ok(guard) => Ok(guard.current_state()),
            Err(_) => Err(DatabaseError::BlockNotFound(
                BlockId::latest(),
            )),
        }
    }

    fn state_by_block_hash(&self, hash: B256) -> Result<Option<Self::State>, DatabaseError> {
        let states = self.states().read();
        if let Some(state) = states.get_state(&hash) {
            let snapshot = state.read_as_state_snapshot();
            let mut new_state = StateDb::new(foundry_evm::backend::MemDb::default());
            new_state.init_from_state_snapshot(snapshot);
            return Ok(Some(new_state));
        }
        Ok(None)
    }
}
