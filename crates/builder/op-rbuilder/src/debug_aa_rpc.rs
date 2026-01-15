//! Debug RPC methods for Account Abstraction (ERC-7769 debug namespace)
//!
//! These methods are for testing/debugging only and should NOT be enabled in production.
//! See: https://eips.ethereum.org/EIPS/eip-7769#rpc-methods-debug-namespace

use alloy_primitives::{Address, Bytes, U256};
use base_account_abstraction::mempool::{p2p::UserOpGossipHandle, UserOpPool};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Debug UserOperation format (v0.6 style for simplicity)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugUserOperation {
    pub sender: Address,
    pub nonce: U256,
    #[serde(default)]
    pub init_code: Bytes,
    pub call_data: Bytes,
    pub call_gas_limit: U256,
    pub verification_gas_limit: U256,
    pub pre_verification_gas: U256,
    pub max_fee_per_gas: U256,
    pub max_priority_fee_per_gas: U256,
    #[serde(default)]
    pub paymaster_and_data: Bytes,
    pub signature: Bytes,
}

impl From<DebugUserOperation> for base_account_abstraction::UserOperationV06 {
    fn from(op: DebugUserOperation) -> Self {
        Self {
            sender: op.sender,
            nonce: op.nonce,
            init_code: op.init_code,
            call_data: op.call_data,
            call_gas_limit: op.call_gas_limit,
            verification_gas_limit: op.verification_gas_limit,
            pre_verification_gas: op.pre_verification_gas,
            max_fee_per_gas: op.max_fee_per_gas,
            max_priority_fee_per_gas: op.max_priority_fee_per_gas,
            paymaster_and_data: op.paymaster_and_data,
            signature: op.signature,
        }
    }
}

/// Peer information returned by getPeers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugPeerInfo {
    pub peer_id: String,
    pub protocols: Vec<String>,
}

/// Node information returned by getNodeInfo
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugNodeInfo {
    pub peer_id: String,
    pub listen_addrs: Vec<String>,
    pub connected_peers: usize,
}

/// Debug RPC API for Account Abstraction bundler (ERC-7769)
#[rpc(server, namespace = "debug_bundler")]
pub trait DebugBundlerApi {
    /// Inject UserOperation objects array into the mempool without validation.
    /// This is for testing/debugging only.
    #[method(name = "addUserOps")]
    async fn add_user_ops(
        &self,
        ops: Vec<DebugUserOperation>,
        entry_point: Address,
    ) -> RpcResult<String>;

    /// Dump the current UserOperation mempool for a given entry point.
    #[method(name = "dumpMempool")]
    async fn dump_mempool(&self, entry_point: Address) -> RpcResult<Vec<DebugUserOperation>>;

    /// Clear the bundler mempool.
    #[method(name = "clearState")]
    async fn clear_state(&self) -> RpcResult<String>;

    /// Connect to a peer at the given multiaddress.
    /// Address format: /ip4/<ip>/tcp/<port>/p2p/<peer_id>
    #[method(name = "connectPeer")]
    async fn connect_peer(&self, multiaddr: String) -> RpcResult<String>;

    /// List all connected peers.
    #[method(name = "getPeers")]
    async fn get_peers(&self) -> RpcResult<Vec<DebugPeerInfo>>;

    /// Get this node's p2p information (peer ID, listen addresses).
    #[method(name = "getNodeInfo")]
    async fn get_node_info(&self) -> RpcResult<DebugNodeInfo>;
}

/// Implementation of the debug bundler RPC
pub struct DebugBundlerRpc {
    pool: Arc<RwLock<UserOpPool>>,
    chain_id: u64,
    gossip_handle: Option<UserOpGossipHandle>,
}

impl DebugBundlerRpc {
    pub fn new(pool: Arc<RwLock<UserOpPool>>, chain_id: u64) -> Self {
        Self {
            pool,
            chain_id,
            gossip_handle: None,
        }
    }

    /// Set the gossip handle for peer management RPCs
    pub fn with_gossip_handle(mut self, handle: UserOpGossipHandle) -> Self {
        self.gossip_handle = Some(handle);
        self
    }
}

#[async_trait::async_trait]
impl DebugBundlerApiServer for DebugBundlerRpc {
    async fn add_user_ops(
        &self,
        ops: Vec<DebugUserOperation>,
        entry_point: Address,
    ) -> RpcResult<String> {
        use base_account_abstraction::UserOperation;

        let mut pool = self.pool.write();
        let mut added = 0;

        for op in ops {
            let user_op = UserOperation::V06(op.into());
            
            // Compute the correct hash for this UserOp
            let hash = user_op.hash(entry_point, self.chain_id);

            // Add directly to pool without validation (debug mode)
            match pool.add_from_peer(user_op, entry_point, hash) {
                Ok(hash) => {
                    tracing::info!(
                        target: "debug_bundler",
                        hash = %hash,
                        entry_point = %entry_point,
                        "Added UserOp via debug RPC"
                    );
                    added += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        target: "debug_bundler",
                        error = %e,
                        "Failed to add UserOp via debug RPC"
                    );
                }
            }
        }

        Ok(format!("ok: added {} ops", added))
    }

    async fn dump_mempool(&self, entry_point: Address) -> RpcResult<Vec<DebugUserOperation>> {
        let pool = self.pool.read();
        let ops = pool.get_best_userops(&entry_point, 1000, u64::MAX);

        let debug_ops: Vec<DebugUserOperation> = ops
            .into_iter()
            .filter_map(|pooled| {
                match &pooled.user_op {
                    base_account_abstraction::UserOperation::V06(op) => Some(DebugUserOperation {
                        sender: op.sender,
                        nonce: op.nonce,
                        init_code: op.init_code.clone(),
                        call_data: op.call_data.clone(),
                        call_gas_limit: op.call_gas_limit,
                        verification_gas_limit: op.verification_gas_limit,
                        pre_verification_gas: op.pre_verification_gas,
                        max_fee_per_gas: op.max_fee_per_gas,
                        max_priority_fee_per_gas: op.max_priority_fee_per_gas,
                        paymaster_and_data: op.paymaster_and_data.clone(),
                        signature: op.signature.clone(),
                    }),
                    _ => None, // Skip v0.7 for now
                }
            })
            .collect();

        Ok(debug_ops)
    }

    async fn clear_state(&self) -> RpcResult<String> {
        let mut pool = self.pool.write();
        pool.clear();
        tracing::info!(target: "debug_bundler", "Cleared AA mempool via debug RPC");
        Ok("ok".to_string())
    }

    async fn connect_peer(&self, multiaddr: String) -> RpcResult<String> {
        let Some(handle) = &self.gossip_handle else {
            return Err(jsonrpsee::types::ErrorObject::owned(
                -32601,
                "P2P gossip not enabled",
                None::<()>,
            ));
        };

        let addr: p2p::Multiaddr = multiaddr.parse().map_err(|e| {
            jsonrpsee::types::ErrorObject::owned(
                -32602,
                format!("Invalid multiaddr: {e}"),
                None::<()>,
            )
        })?;

        match handle.dial_peer(addr).await {
            Ok(peer_id) => {
                tracing::info!(target: "debug_bundler", peer_id = %peer_id, "Dialing peer via RPC");
                Ok(format!("dialing peer: {peer_id}"))
            }
            Err(e) => Err(jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("Failed to dial peer: {e}"),
                None::<()>,
            )),
        }
    }

    async fn get_peers(&self) -> RpcResult<Vec<DebugPeerInfo>> {
        let Some(handle) = &self.gossip_handle else {
            return Err(jsonrpsee::types::ErrorObject::owned(
                -32601,
                "P2P gossip not enabled",
                None::<()>,
            ));
        };

        match handle.list_peers().await {
            Ok(peers) => {
                let debug_peers: Vec<DebugPeerInfo> = peers
                    .into_iter()
                    .map(|p| DebugPeerInfo {
                        peer_id: p.peer_id,
                        protocols: p.protocols,
                    })
                    .collect();
                Ok(debug_peers)
            }
            Err(e) => Err(jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("Failed to list peers: {e}"),
                None::<()>,
            )),
        }
    }

    async fn get_node_info(&self) -> RpcResult<DebugNodeInfo> {
        let Some(handle) = &self.gossip_handle else {
            return Err(jsonrpsee::types::ErrorObject::owned(
                -32601,
                "P2P gossip not enabled",
                None::<()>,
            ));
        };

        match handle.get_node_info().await {
            Ok(info) => Ok(DebugNodeInfo {
                peer_id: info.peer_id,
                listen_addrs: info.listen_addrs,
                connected_peers: info.connected_peers,
            }),
            Err(e) => Err(jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("Failed to get node info: {e}"),
                None::<()>,
            )),
        }
    }
}
