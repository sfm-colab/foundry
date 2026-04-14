//! `AnvilProvider` — adapts anvil's `Backend` to reth's storage provider traits.

use crate::{backend::BackendView, convert};
use alloy_consensus::Header;
use alloy_eips::{BlockHashOrNumber, BlockId, BlockNumHash, BlockNumberOrTag};
use alloy_primitives::{
    Address, BlockHash, BlockNumber, Bytes, StorageKey, StorageValue, TxHash, TxNumber, B256,
};
use foundry_evm::backend::DatabaseError;
use reth_chain_state::{CanonStateNotifications, CanonStateSubscriptions};
use reth_chainspec::{ChainInfo, ChainSpecProvider};
use reth_db_models::StoredBlockBodyIndices;
use reth_ethereum_primitives::EthPrimitives;
use reth_execution_types::ExecutionOutcome;
use reth_primitives_traits::{Account, Bytecode, NodePrimitives, RecoveredBlock, SealedHeader};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    AccountReader, BlockBodyIndicesProvider, BlockHashReader, BlockIdReader, BlockNumReader,
    BlockReader, BlockReaderIdExt, BlockSource, BytecodeReader, ChangeSetReader,
    HashedPostStateProvider, HeaderProvider, NodePrimitivesProvider, ReceiptProvider,
    ReceiptProviderIdExt, StageCheckpointReader, StateProofProvider, StateProvider,
    StateProviderBox, StateProviderFactory, StateReader, StateRootProvider, StorageRootProvider,
    TransactionVariant, TransactionsProvider,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie_common::{
    updates::TrieUpdates, AccountProof, ExecutionWitnessMode, HashedPostState, HashedStorage,
    MultiProof, MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use revm::database::DatabaseRef;
use std::{
    fmt,
    ops::{RangeBounds, RangeInclusive},
    sync::Arc,
};
use tokio::sync::broadcast;

use alloy_consensus::transaction::TransactionMeta;
use reth_db_models::AccountBeforeTx;

type AnvilBlock = anvil_core::eth::block::Block;

/// Provider that wraps a [`BackendView`] implementor and implements reth's storage traits.
pub struct AnvilProvider<B: BackendView> {
    backend: Arc<B>,
    chain_spec: Arc<reth_chainspec::ChainSpec>,
    canon_state_tx: broadcast::Sender<reth_chain_state::CanonStateNotification<EthPrimitives>>,
}

impl<B: BackendView> Clone for AnvilProvider<B> {
    fn clone(&self) -> Self {
        Self {
            backend: self.backend.clone(),
            chain_spec: self.chain_spec.clone(),
            canon_state_tx: self.canon_state_tx.clone(),
        }
    }
}

impl<B: BackendView> AnvilProvider<B> {
    pub fn new(backend: Arc<B>, chain_spec: Arc<reth_chainspec::ChainSpec>) -> Self {
        let (canon_state_tx, _) = broadcast::channel(16);
        Self { backend, chain_spec, canon_state_tx }
    }

    /// Resolve a block hash from the backend.
    fn resolve_block_hash(&self, id: BlockHashOrNumber) -> Option<B256> {
        match id {
            BlockHashOrNumber::Hash(h) => {
                if self.backend.has_block(h) {
                    Some(h)
                } else {
                    None
                }
            }
            BlockHashOrNumber::Number(n) => self.backend.number_to_hash(n),
        }
    }

    /// Get a block from storage by hash.
    fn get_block(&self, hash: &B256) -> Option<AnvilBlock> {
        self.backend.block_by_hash(*hash)
    }
}

impl<B: BackendView> fmt::Debug for AnvilProvider<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnvilProvider").finish_non_exhaustive()
    }
}

// =============================================================================
// NodePrimitivesProvider
// =============================================================================

impl<B: BackendView> NodePrimitivesProvider for AnvilProvider<B> {
    type Primitives = EthPrimitives;
}

// =============================================================================
// BlockHashReader
// =============================================================================

impl<B: BackendView> BlockHashReader for AnvilProvider<B> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        Ok(self.backend.number_to_hash(number))
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        let mut hashes = Vec::new();
        for n in start..end {
            if let Some(h) = self.backend.number_to_hash(n) {
                hashes.push(h);
            }
        }
        Ok(hashes)
    }
}

// =============================================================================
// BlockNumReader
// =============================================================================

impl<B: BackendView> BlockNumReader for AnvilProvider<B> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        Ok(ChainInfo {
            best_hash: self.backend.best_hash(),
            best_number: self.backend.best_number(),
        })
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(self.backend.best_number())
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(self.backend.best_number())
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        Ok(self.backend.block_by_hash(hash).map(|b| b.header.number))
    }
}

// =============================================================================
// BlockIdReader
// =============================================================================

impl<B: BackendView> BlockIdReader for AnvilProvider<B> {
    fn pending_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        Ok(None)
    }

    fn safe_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        let hash = self.backend.block_hash(BlockNumberOrTag::Safe);
        hash.map(|h| {
            let number =
                self.backend.block_by_hash(h).map(|b| b.header.number).unwrap_or(0);
            Ok(BlockNumHash { number, hash: h })
        })
        .transpose()
    }

    fn finalized_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        let hash = self.backend.block_hash(BlockNumberOrTag::Finalized);
        hash.map(|h| {
            let number =
                self.backend.block_by_hash(h).map(|b| b.header.number).unwrap_or(0);
            Ok(BlockNumHash { number, hash: h })
        })
        .transpose()
    }
}

// =============================================================================
// ChainSpecProvider
// =============================================================================

impl<B: BackendView> ChainSpecProvider for AnvilProvider<B> {
    type ChainSpec = reth_chainspec::ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.chain_spec.clone()
    }
}

// =============================================================================
// HeaderProvider
// =============================================================================

impl<B: BackendView> HeaderProvider for AnvilProvider<B> {
    type Header = Header;

    fn header(&self, block_hash: BlockHash) -> ProviderResult<Option<Self::Header>> {
        Ok(self.get_block(&block_hash).map(|b| b.header.clone()))
    }

    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Self::Header>> {
        match self.backend.number_to_hash(num) {
            Some(h) => self.header(h),
            None => Ok(None),
        }
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        let best_number = self.backend.best_number();
        let mut headers = Vec::new();
        let start = match range.start_bound() {
            std::ops::Bound::Included(&n) => n,
            std::ops::Bound::Excluded(&n) => n + 1,
            std::ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            std::ops::Bound::Included(&n) => n + 1,
            std::ops::Bound::Excluded(&n) => n,
            std::ops::Bound::Unbounded => best_number + 1,
        };
        for n in start..end {
            if let Some(h) = self.backend.number_to_hash(n) {
                if let Some(block) = self.backend.block_by_hash(h) {
                    headers.push(block.header.clone());
                }
            }
        }
        Ok(headers)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        if let Some(hash) = self.backend.number_to_hash(number) {
            if let Some(block) = self.backend.block_by_hash(hash) {
                return Ok(Some(SealedHeader::new(block.header.clone(), hash)));
            }
        }
        Ok(None)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        let best_number = self.backend.best_number();
        let mut predicate = predicate;
        let mut headers = Vec::new();
        let start = match range.start_bound() {
            std::ops::Bound::Included(&n) => n,
            std::ops::Bound::Excluded(&n) => n + 1,
            std::ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            std::ops::Bound::Included(&n) => n + 1,
            std::ops::Bound::Excluded(&n) => n,
            std::ops::Bound::Unbounded => best_number + 1,
        };
        for n in start..end {
            if let Some(hash) = self.backend.number_to_hash(n) {
                if let Some(block) = self.backend.block_by_hash(hash) {
                    let sealed = SealedHeader::new(block.header.clone(), hash);
                    if !predicate(&sealed) {
                        break;
                    }
                    headers.push(sealed);
                }
            }
        }
        Ok(headers)
    }
}

// =============================================================================
// BlockReader
// =============================================================================

impl<B: BackendView> BlockReader for AnvilProvider<B> {
    type Block = <EthPrimitives as NodePrimitives>::Block;

    fn find_block_by_hash(
        &self,
        hash: B256,
        _source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        Ok(self
            .get_block(&hash)
            .map(|b| convert::convert_block(&b).0.into_block()))
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        let hash = match self.resolve_block_hash(id) {
            Some(h) => h,
            None => return Ok(None),
        };
        self.find_block_by_hash(hash, BlockSource::Any)
    }

    fn pending_block(&self) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Ok(None)
    }

    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<
        Option<(
            RecoveredBlock<Self::Block>,
            Vec<<EthPrimitives as NodePrimitives>::Receipt>,
        )>,
    > {
        Ok(None)
    }

    fn recovered_block(
        &self,
        id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        let hash = match self.resolve_block_hash(id) {
            Some(h) => h,
            None => return Ok(None),
        };
        let anvil_block = match self.get_block(&hash) {
            Some(b) => b,
            None => return Ok(None),
        };
        let (sealed_block, senders) = convert::convert_block(&anvil_block);
        Ok(Some(RecoveredBlock::new_sealed(sealed_block, senders)))
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.recovered_block(id, transaction_kind)
    }

    fn block_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Block>> {
        let mut blocks = Vec::new();
        for n in range {
            if let Some(hash) = self.backend.number_to_hash(n) {
                if let Some(anvil_block) = self.backend.block_by_hash(hash) {
                    blocks.push(convert::convert_block(&anvil_block).0.into_block());
                }
            }
        }
        Ok(blocks)
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.recovered_block_range(range)
    }

    fn recovered_block_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        let mut blocks = Vec::new();
        for n in range {
            if let Some(hash) = self.backend.number_to_hash(n) {
                if let Some(anvil_block) = self.backend.block_by_hash(hash) {
                    let (sealed_block, senders) = convert::convert_block(&anvil_block);
                    blocks.push(RecoveredBlock::new_sealed(sealed_block, senders));
                }
            }
        }
        Ok(blocks)
    }

    fn block_by_transaction_id(&self, _id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        Ok(None)
    }
}

impl<B: BackendView> BlockReaderIdExt for AnvilProvider<B> {
    fn block_by_id(&self, id: BlockId) -> ProviderResult<Option<Self::Block>> {
        match id {
            BlockId::Hash(hash) => self.block(BlockHashOrNumber::Hash(hash.block_hash)),
            BlockId::Number(num) => {
                match self.backend.block_hash(num) {
                    Some(h) => self.block(BlockHashOrNumber::Hash(h)),
                    None => Ok(None),
                }
            }
        }
    }

    fn sealed_header_by_id(
        &self,
        id: BlockId,
    ) -> ProviderResult<Option<SealedHeader<Header>>> {
        match id {
            BlockId::Hash(hash) => {
                let block = self.get_block(&hash.block_hash);
                Ok(block.map(|b| SealedHeader::new(b.header.clone(), hash.block_hash)))
            }
            BlockId::Number(num) => {
                match self.backend.block_hash(num) {
                    Some(h) => Ok(self
                        .backend
                        .block_by_hash(h)
                        .map(|b| SealedHeader::new(b.header.clone(), h))),
                    None => Ok(None),
                }
            }
        }
    }

    fn header_by_id(&self, id: BlockId) -> ProviderResult<Option<Header>> {
        match id {
            BlockId::Hash(hash) => self.header(hash.block_hash),
            BlockId::Number(num) => {
                match self.backend.block_hash(num) {
                    Some(h) => self.header(h),
                    None => Ok(None),
                }
            }
        }
    }
}

// =============================================================================
// TransactionsProvider
// =============================================================================

impl<B: BackendView> TransactionsProvider for AnvilProvider<B> {
    type Transaction = <EthPrimitives as NodePrimitives>::SignedTx;

    fn transaction_id(&self, _tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        Ok(None)
    }

    fn transaction_by_id(&self, _id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        Ok(None)
    }

    fn transaction_by_id_unhashed(
        &self,
        _id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        Ok(None)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        if let Some(mined) = self.backend.mined_transaction_by_hash(hash) {
            if let Some(block) = self.backend.block_by_hash(mined.block_hash) {
                if let Some(tx) = block.body.transactions.get(mined.info.transaction_index as usize)
                {
                    if let Some((reth_tx, _)) = convert::convert_tx(tx) {
                        return Ok(Some(reth_tx));
                    }
                }
            }
        }
        Ok(None)
    }

    fn transaction_by_hash_with_meta(
        &self,
        hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        if let Some(mined) = self.backend.mined_transaction_by_hash(hash) {
            if let Some(block) = self.backend.block_by_hash(mined.block_hash) {
                if let Some(tx) = block.body.transactions.get(mined.info.transaction_index as usize)
                {
                    if let Some((reth_tx, _)) = convert::convert_tx(tx) {
                        let meta = TransactionMeta {
                            tx_hash: hash,
                            index: mined.info.transaction_index,
                            block_hash: mined.block_hash,
                            block_number: mined.block_number,
                            base_fee: block.header.base_fee_per_gas,
                            excess_blob_gas: block.header.excess_blob_gas,
                            timestamp: block.header.timestamp,
                        };
                        return Ok(Some((reth_tx, meta)));
                    }
                }
            }
        }
        Ok(None)
    }

    fn transactions_by_block(
        &self,
        block_id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        let hash = match self.resolve_block_hash(block_id) {
            Some(h) => h,
            None => return Ok(None),
        };
        if let Some(block) = self.backend.block_by_hash(hash) {
            let txs: Vec<_> = block
                .body
                .transactions
                .iter()
                .filter_map(|tx| convert::convert_tx(tx).map(|(t, _)| t))
                .collect();
            return Ok(Some(txs));
        }
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        Ok(Vec::new())
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        Ok(Vec::new())
    }

    fn senders_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        Ok(Vec::new())
    }

    fn transaction_sender(&self, _id: TxNumber) -> ProviderResult<Option<Address>> {
        Ok(None)
    }
}

// =============================================================================
// ReceiptProvider
// =============================================================================

impl<B: BackendView> ReceiptProvider for AnvilProvider<B> {
    type Receipt = <EthPrimitives as NodePrimitives>::Receipt;

    fn receipt(&self, _id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        Ok(None)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        if let Some(mined) = self.backend.mined_transaction_by_hash(hash) {
            return Ok(convert::convert_receipt_view(&mined.receipt));
        }
        Ok(None)
    }

    fn receipts_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        let hash = match self.resolve_block_hash(block) {
            Some(h) => h,
            None => return Ok(None),
        };
        let tx_hashes = self.backend.block_transaction_hashes(hash);
        if tx_hashes.is_empty() && !self.backend.has_block(hash) {
            return Ok(None);
        }
        let receipts: Vec<_> = tx_hashes
            .iter()
            .filter_map(|tx_hash| {
                self.backend
                    .mined_transaction_by_hash(*tx_hash)
                    .and_then(|mined| convert::convert_receipt_view(&mined.receipt))
            })
            .collect();
        Ok(Some(receipts))
    }

    fn receipts_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        Ok(Vec::new())
    }

    fn receipts_by_block_range(
        &self,
        _block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Receipt>>> {
        Ok(Vec::new())
    }
}

impl<B: BackendView> ReceiptProviderIdExt for AnvilProvider<B> {}

// =============================================================================
// AccountReader
// =============================================================================

impl<B: BackendView> AccountReader for AnvilProvider<B> {
    fn basic_account(&self, _address: &Address) -> ProviderResult<Option<Account>> {
        Ok(None)
    }
}

// =============================================================================
// ChangeSetReader
// =============================================================================

impl<B: BackendView> ChangeSetReader for AnvilProvider<B> {
    fn account_block_changeset(
        &self,
        _block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        Ok(Vec::new())
    }

    fn get_account_before_block(
        &self,
        _block_number: BlockNumber,
        _address: Address,
    ) -> ProviderResult<Option<AccountBeforeTx>> {
        Ok(None)
    }

    fn account_changesets_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(BlockNumber, AccountBeforeTx)>> {
        Ok(Vec::new())
    }
}

// =============================================================================
// StateProviderFactory
// =============================================================================

impl<B: BackendView> StateProviderFactory for AnvilProvider<B> {
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        match self.backend.latest_state() {
            Ok(state) => Ok(Box::new(AnvilStateProvider::new(state))),
            Err(e) => Err(ProviderError::other(e)),
        }
    }

    fn state_by_block_number_or_tag(
        &self,
        number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox> {
        match number_or_tag {
            BlockNumberOrTag::Latest | BlockNumberOrTag::Pending => self.latest(),
            BlockNumberOrTag::Earliest => self.history_by_block_number(0),
            BlockNumberOrTag::Number(num) => self.history_by_block_number(num),
            BlockNumberOrTag::Finalized => {
                let hash =
                    self.finalized_block_hash()?.ok_or(ProviderError::FinalizedBlockNotFound)?;
                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Safe => {
                let hash = self.safe_block_hash()?.ok_or(ProviderError::SafeBlockNotFound)?;
                self.history_by_block_hash(hash)
            }
        }
    }

    fn history_by_block_number(&self, block: BlockNumber) -> ProviderResult<StateProviderBox> {
        match self.backend.number_to_hash(block) {
            Some(h) => self.history_by_block_hash(h),
            None => Err(ProviderError::BlockHashNotFound(B256::ZERO)),
        }
    }

    fn history_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox> {
        if block == self.backend.best_hash() {
            return self.latest();
        }

        match self.backend.state_by_block_hash(block) {
            Ok(Some(state)) => Ok(Box::new(AnvilStateProvider::new(state))),
            Ok(None) => Err(ProviderError::StateForHashNotFound(block)),
            Err(e) => Err(ProviderError::other(e)),
        }
    }

    fn state_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox> {
        self.history_by_block_hash(block)
    }

    fn pending(&self) -> ProviderResult<StateProviderBox> {
        self.latest()
    }

    fn pending_state_by_hash(
        &self,
        _block_hash: B256,
    ) -> ProviderResult<Option<StateProviderBox>> {
        Ok(None)
    }

    fn maybe_pending(&self) -> ProviderResult<Option<StateProviderBox>> {
        Ok(None)
    }
}

// =============================================================================
// StateReader
// =============================================================================

impl<B: BackendView> StateReader for AnvilProvider<B> {
    type Receipt = <EthPrimitives as NodePrimitives>::Receipt;

    fn get_state(
        &self,
        _block: BlockNumber,
    ) -> ProviderResult<Option<ExecutionOutcome<Self::Receipt>>> {
        Ok(None)
    }
}

// =============================================================================
// StageCheckpointReader
// =============================================================================

impl<B: BackendView> StageCheckpointReader for AnvilProvider<B> {
    fn get_stage_checkpoint(&self, _id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        Ok(None)
    }

    fn get_stage_checkpoint_progress(&self, _id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        Ok(Vec::new())
    }
}

// =============================================================================
// BlockBodyIndicesProvider
// =============================================================================

impl<B: BackendView> BlockBodyIndicesProvider for AnvilProvider<B> {
    fn block_body_indices(
        &self,
        _num: u64,
    ) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        Ok(None)
    }

    fn block_body_indices_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<StoredBlockBodyIndices>> {
        Ok(Vec::new())
    }
}

// =============================================================================
// CanonStateSubscriptions
// =============================================================================

impl<B: BackendView> CanonStateSubscriptions for AnvilProvider<B> {
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications<Self::Primitives> {
        self.canon_state_tx.subscribe()
    }
}

// =============================================================================
// AnvilStateProvider — wraps a BackendView::State for sync StateProvider access
// =============================================================================

/// Wraps a state snapshot implementing `DatabaseRef` to implement reth's `StateProvider`.
pub struct AnvilStateProvider<S: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync>(S);

impl<S: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync> AnvilStateProvider<S> {
    pub fn new(state: S) -> Self {
        Self(state)
    }
}

impl<S: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync> fmt::Debug
    for AnvilStateProvider<S>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnvilStateProvider").finish_non_exhaustive()
    }
}

fn db_err_to_provider(e: DatabaseError) -> ProviderError {
    ProviderError::other(e)
}

impl<S: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync> BlockHashReader
    for AnvilStateProvider<S>
{
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        match self.0.block_hash_ref(number) {
            Ok(h) if h == B256::ZERO => Ok(None),
            Ok(h) => Ok(Some(h)),
            Err(e) => Err(db_err_to_provider(e)),
        }
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        Ok(Vec::new())
    }
}

impl<S: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync> AccountReader
    for AnvilStateProvider<S>
{
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        match self.0.basic_ref(*address) {
            Ok(Some(info)) => Ok(Some(Account {
                nonce: info.nonce,
                balance: info.balance,
                bytecode_hash: if info.code_hash == revm::primitives::KECCAK_EMPTY {
                    None
                } else {
                    Some(info.code_hash)
                },
            })),
            Ok(None) => Ok(None),
            Err(e) => Err(db_err_to_provider(e)),
        }
    }
}

impl<S: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync> BytecodeReader
    for AnvilStateProvider<S>
{
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        if *code_hash == revm::primitives::KECCAK_EMPTY {
            return Ok(None);
        }
        match self.0.code_by_hash_ref(*code_hash) {
            Ok(code) => Ok(Some(Bytecode(code))),
            Err(e) => Err(db_err_to_provider(e)),
        }
    }
}

impl<S: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync> StateProvider
    for AnvilStateProvider<S>
{
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        match self.0.storage_ref(account, storage_key.into()) {
            Ok(val) if val.is_zero() => Ok(None),
            Ok(val) => Ok(Some(val)),
            Err(e) => Err(db_err_to_provider(e)),
        }
    }
}

// Noop trie/proof impls required by StateProvider supertrait bounds

impl<S: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync> StateRootProvider
    for AnvilStateProvider<S>
{
    fn state_root(&self, _state: HashedPostState) -> ProviderResult<B256> {
        Ok(B256::ZERO)
    }

    fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
        Ok(B256::ZERO)
    }

    fn state_root_with_updates(
        &self,
        _state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        Ok((B256::ZERO, TrieUpdates::default()))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        _input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        Ok((B256::ZERO, TrieUpdates::default()))
    }
}

impl<S: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync> StorageRootProvider
    for AnvilStateProvider<S>
{
    fn storage_root(
        &self,
        _address: Address,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        Ok(B256::ZERO)
    }

    fn storage_proof(
        &self,
        _address: Address,
        slot: B256,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        Ok(StorageProof::new(slot))
    }

    fn storage_multiproof(
        &self,
        _address: Address,
        _slots: &[B256],
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        Ok(StorageMultiProof::empty())
    }
}

impl<S: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync> StateProofProvider
    for AnvilStateProvider<S>
{
    fn proof(
        &self,
        _input: TrieInput,
        address: Address,
        _slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        Ok(AccountProof::new(address))
    }

    fn multiproof(
        &self,
        _input: TrieInput,
        _targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        Ok(MultiProof::default())
    }

    fn witness(
        &self,
        _input: TrieInput,
        _target: HashedPostState,
        _mode: ExecutionWitnessMode,
    ) -> ProviderResult<Vec<Bytes>> {
        Ok(Vec::new())
    }
}

impl<S: DatabaseRef<Error = DatabaseError> + fmt::Debug + Send + Sync> HashedPostStateProvider
    for AnvilStateProvider<S>
{
    fn hashed_post_state(
        &self,
        _bundle_state: &revm_database::BundleState,
    ) -> HashedPostState {
        HashedPostState::default()
    }
}
