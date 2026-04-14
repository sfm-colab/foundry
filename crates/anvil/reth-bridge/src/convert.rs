//! Type conversion between anvil's types and reth's `EthPrimitives`.

use crate::backend::ReceiptView;
use alloy_consensus::TxEnvelope;
use alloy_primitives::Address;
use anvil_core::eth::{block::Block as AnvilBlock, transaction::MaybeImpersonatedTransaction};
use foundry_primitives::{FoundryReceiptEnvelope, FoundryTxEnvelope};
use reth_ethereum_primitives::{
    Block as RethBlock, BlockBody as RethBlockBody, Receipt as RethReceipt, TransactionSigned,
};
use reth_primitives_traits::SealedBlock;

/// Converts a `MaybeImpersonatedTransaction<FoundryTxEnvelope>` to reth's `TransactionSigned`,
/// returning the recovered sender.
///
/// Returns `None` for Deposit and Tempo transactions (not standard Ethereum types).
pub fn convert_tx(
    tx: &MaybeImpersonatedTransaction<FoundryTxEnvelope>,
) -> Option<(TransactionSigned, Address)> {
    let sender = tx.recover().ok()?;
    // Deref gives us &FoundryTxEnvelope
    let inner: &FoundryTxEnvelope = tx;
    let eth_envelope: TxEnvelope = inner.clone().try_into_eth().ok()?;
    let reth_tx: TransactionSigned = eth_envelope.into();
    Some((reth_tx, sender))
}

/// Converts a receipt implementing [`ReceiptView`] to reth's `Receipt`.
///
/// Returns `None` for non-standard receipt types (Deposit, Tempo).
pub fn convert_receipt_view<R: ReceiptView>(receipt: &R) -> Option<RethReceipt> {
    let tx_type = receipt.tx_type()?;
    Some(RethReceipt {
        tx_type,
        success: receipt.success(),
        cumulative_gas_used: receipt.cumulative_gas_used(),
        logs: receipt.logs(),
    })
}

/// Converts a `FoundryReceiptEnvelope` to reth's `Receipt`.
///
/// Returns `None` for Deposit and Tempo receipts.
pub fn convert_receipt(receipt: &FoundryReceiptEnvelope) -> Option<RethReceipt> {
    let (tx_type, inner) = match receipt {
        FoundryReceiptEnvelope::Legacy(r) => (alloy_consensus::TxType::Legacy, &r.receipt),
        FoundryReceiptEnvelope::Eip2930(r) => (alloy_consensus::TxType::Eip2930, &r.receipt),
        FoundryReceiptEnvelope::Eip1559(r) => (alloy_consensus::TxType::Eip1559, &r.receipt),
        FoundryReceiptEnvelope::Eip4844(r) => (alloy_consensus::TxType::Eip4844, &r.receipt),
        FoundryReceiptEnvelope::Eip7702(r) => (alloy_consensus::TxType::Eip7702, &r.receipt),
        FoundryReceiptEnvelope::Deposit(_) | FoundryReceiptEnvelope::Tempo(_) => return None,
    };

    Some(RethReceipt {
        tx_type,
        success: inner.status.coerce_status(),
        cumulative_gas_used: inner.cumulative_gas_used,
        logs: inner.logs.clone(),
    })
}

/// Converts an anvil `Block` to a reth `SealedBlock` plus a list of senders.
///
/// Transactions that cannot be converted (Deposit, Tempo) are skipped.
pub fn convert_block(block: &AnvilBlock) -> (SealedBlock<RethBlock>, Vec<Address>) {
    let mut reth_txs = Vec::new();
    let mut senders = Vec::new();

    for tx in &block.body.transactions {
        if let Some((reth_tx, sender)) = convert_tx(tx) {
            reth_txs.push(reth_tx);
            senders.push(sender);
        }
    }

    let reth_body = RethBlockBody {
        transactions: reth_txs,
        ommers: Vec::new(),
        withdrawals: block.body.withdrawals.clone(),
    };

    let reth_block = RethBlock::new(block.header.clone(), reth_body);
    let sealed = SealedBlock::seal_slow(reth_block);
    (sealed, senders)
}
