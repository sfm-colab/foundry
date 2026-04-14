//! Noop network implementation for anvil (local dev node, no p2p).

use reth_network_api::{EthProtocolInfo, NetworkError, NetworkInfo, NetworkStatus};
use std::net::SocketAddr;

/// Noop network adapter. Anvil is a local dev node with no real p2p.
#[derive(Debug, Clone)]
pub struct AnvilNetwork {
    chain_id: u64,
}

impl AnvilNetwork {
    pub fn new(chain_id: u64) -> Self {
        Self { chain_id }
    }
}

impl NetworkInfo for AnvilNetwork {
    fn local_addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 0))
    }

    fn network_status(
        &self,
    ) -> impl std::future::Future<Output = Result<NetworkStatus, NetworkError>> + Send {
        async {
            Ok(NetworkStatus {
                client_version: "anvil/0.0.0".to_string(),
                protocol_version: 0,
                eth_protocol_info: EthProtocolInfo {
                    network: 1,
                    #[allow(deprecated)]
                    difficulty: None,
                    genesis: Default::default(),
                    config: Default::default(),
                    head: Default::default(),
                },
                capabilities: Vec::new(),
            })
        }
    }

    fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn is_syncing(&self) -> bool {
        false
    }

    fn is_initially_syncing(&self) -> bool {
        false
    }
}
