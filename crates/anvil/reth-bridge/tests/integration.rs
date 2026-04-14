//! Smoke tests: spin up anvil, create AnvilProvider, verify reads work.

use anvil::{spawn, NodeConfig};
use anvil::eth::backend::mem::Backend;
use anvil_reth_bridge::AnvilProvider;
use foundry_primitives::FoundryNetwork;
use reth_storage_api::{
    AccountReader, BlockNumReader, BlockReader, HeaderProvider, ReceiptProvider,
    StateProviderFactory, TransactionsProvider,
};
use std::sync::Arc;

fn make_provider(
    backend: Arc<Backend<FoundryNetwork>>,
) -> AnvilProvider<Backend<FoundryNetwork>> {
    let chain_spec = Arc::new(reth_chainspec::ChainSpec::default());
    AnvilProvider::new(backend, chain_spec)
}

#[tokio::test]
async fn test_block_num_reader() {
    let (api, _handle) = spawn(NodeConfig::test()).await;
    let provider = make_provider(api.backend.clone());

    let best = provider.best_block_number().unwrap();
    assert_eq!(best, 0, "fresh anvil should be at block 0");

    let info = provider.chain_info().unwrap();
    assert_eq!(info.best_number, 0);
    assert_ne!(info.best_hash, alloy_primitives::B256::ZERO, "genesis hash should not be zero");
}

#[tokio::test]
async fn test_header_provider() {
    let (api, _handle) = spawn(NodeConfig::test()).await;
    let provider = make_provider(api.backend.clone());

    let header = provider.header_by_number(0).unwrap();
    assert!(header.is_some(), "genesis header should exist");
    assert_eq!(header.unwrap().number, 0);

    let missing = provider.header_by_number(999).unwrap();
    assert!(missing.is_none(), "non-existent block should return None");
}

#[tokio::test]
async fn test_block_reader() {
    let (api, _handle) = spawn(NodeConfig::test()).await;
    let provider = make_provider(api.backend.clone());

    // Read genesis block
    let block = provider.block_by_number(0).unwrap();
    assert!(block.is_some(), "genesis block should exist");

    let block = block.unwrap();
    assert_eq!(block.header.number, 0);
    assert!(block.body.transactions.is_empty(), "genesis should have no txs");
}

#[tokio::test]
async fn test_state_provider_latest() {
    let (api, handle) = spawn(NodeConfig::test()).await;
    let provider = make_provider(api.backend.clone());

    let state = provider.latest().unwrap();

    // Dev accounts should have balances
    let dev_addr = handle.dev_accounts().next().unwrap();
    let account = state.basic_account(&dev_addr).unwrap();
    assert!(account.is_some(), "dev account should exist");

    let account = account.unwrap();
    assert!(account.balance > alloy_primitives::U256::ZERO, "dev account should have balance");
    assert_eq!(account.nonce, 0, "fresh dev account nonce should be 0");
}

#[tokio::test]
async fn test_after_transaction() {
    let (api, handle) = spawn(NodeConfig::test()).await;
    let provider = make_provider(api.backend.clone());

    // Send a transaction to mine a block
    let accounts = handle.dev_accounts().collect::<Vec<_>>();
    let from = accounts[0];
    let to = accounts[1];

    let tx = alloy_rpc_types::TransactionRequest::default()
        .to(to)
        .value(alloy_primitives::U256::from(1000))
        .from(from);

    let tx_hash = api.send_transaction(alloy_serde::WithOtherFields::new(tx)).await.unwrap();

    // Wait for the tx to be mined
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Check block number increased
    let best = provider.best_block_number().unwrap();
    assert!(best >= 1, "block should have been mined, got {best}");

    // Check transaction is findable
    let found_tx = provider.transaction_by_hash(tx_hash).unwrap();
    assert!(found_tx.is_some(), "mined tx should be findable by hash");

    // Check receipt
    let receipt = provider.receipt_by_hash(tx_hash).unwrap();
    assert!(receipt.is_some(), "receipt should exist for mined tx");

    // Check state updated
    let state = provider.latest().unwrap();
    let sender_account = state.basic_account(&from).unwrap().unwrap();
    assert_eq!(sender_account.nonce, 1, "sender nonce should be 1 after tx");
}
