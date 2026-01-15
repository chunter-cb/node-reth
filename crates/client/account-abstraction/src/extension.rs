//! Contains the [`AccountAbstractionExtension`] which wires up the account abstraction
//! RPC surfaces on the Base node builder.

use std::sync::Arc;

use base_account_abstraction_indexer::UserOperationStorage;
use base_client_node::{BaseNodeExtension, FromExtensionConfig, OpBuilder};
use parking_lot::RwLock;
use reth_provider::ChainSpecProvider;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::{
    mempool::{GossipConfig, UserOpGossip, UserOpGossipHandle, UserOpPool},
    AccountAbstractionApiImpl, AccountAbstractionApiServer, AccountAbstractionArgs,
    BaseAccountAbstractionApiImpl, BaseAccountAbstractionApiServer,
};

/// Configuration for the Account Abstraction extension.
#[derive(Debug, Clone)]
pub struct AccountAbstractionConfig {
    /// CLI arguments for account abstraction
    pub args: AccountAbstractionArgs,
}

impl From<AccountAbstractionArgs> for AccountAbstractionConfig {
    fn from(args: AccountAbstractionArgs) -> Self {
        Self { args }
    }
}

/// Helper struct that wires the account abstraction RPC into the node builder.
#[derive(Debug, Clone)]
pub struct AccountAbstractionExtension {
    /// Configuration for the extension
    config: AccountAbstractionConfig,
}

impl AccountAbstractionExtension {
    /// Creates a new account abstraction extension.
    pub fn new(config: AccountAbstractionConfig) -> Self {
        Self { config }
    }
}

impl BaseNodeExtension for AccountAbstractionExtension {
    /// Applies the extension to the supplied builder.
    fn apply(self: Box<Self>, builder: OpBuilder) -> OpBuilder {
        let args = self.config.args.clone();

        if !args.enabled {
            return builder;
        }

        // Validate configuration
        if let Err(e) = args.validate() {
            tracing::error!(error = %e, "Account Abstraction configuration validation failed");
            return builder;
        }

        // Create shared storage for the indexer ExEx
        let storage = if args.indexer_enabled {
            Some(Arc::new(UserOperationStorage::new()))
        } else {
            None
        };

        let storage_for_exex = storage.clone();
        let indexer_enabled = args.indexer_enabled;

        // Install the ExEx if indexer is enabled
        let builder = builder.install_exex_if(indexer_enabled, "aa-indexer", move |ctx| async move {
            let storage = storage_for_exex.expect("storage should be set when indexer is enabled");
            info!(target: "aa", "Starting Account Abstraction UserOperation Indexer ExEx");
            Ok(base_account_abstraction_indexer::account_abstraction_indexer_exex(ctx, storage))
        });

        // Only get TIPS URL if not in mempool mode
        let tips_url = if args.mempool_enabled {
            None
        } else {
            Some(args.send_url())
        };
        let mempool_enabled = args.mempool_enabled;
        let p2p_enabled = args.p2p_enabled;
        let mempool_config = args.mempool_config();
        let gossip_config = args.gossip_config();
        let args_clone = args.clone();

        builder.extend_rpc_modules(move |ctx| {
            info!(target: "aa", "Starting Account Abstraction RPC");

            // Get chain ID for UserOp hash computation
            let chain_id = ctx.provider().chain_spec().chain().id();

            // Create the main eth_ and base_ RPC implementations
            let aa_api = if mempool_enabled {
                // Create mempool
                let pool = UserOpPool::new(mempool_config.clone(), chain_id);
                let mempool = Arc::new(RwLock::new(pool));

                // Start p2p gossip if enabled
                let gossip_handle: Option<UserOpGossipHandle> = if p2p_enabled {
                    if let Some(config) = gossip_config.clone() {
                        match start_gossip_service(config, mempool.clone(), chain_id) {
                            Ok(handle) => {
                                info!(target: "aa", "P2P gossip service started");
                                Some(handle)
                            }
                            Err(e) => {
                                warn!(target: "aa", error = %e, "Failed to start p2p gossip");
                                None
                            }
                        }
                    } else {
                        warn!(target: "aa", "P2P enabled but no gossip config provided");
                        None
                    }
                } else {
                    None
                };

                info!(
                    target: "aa",
                    chain_id = chain_id,
                    p2p_enabled = gossip_handle.is_some(),
                    "Using local mempool mode"
                );
                AccountAbstractionApiImpl::new_with_mempool(
                    ctx.provider().clone(),
                    ctx.registry.eth_api().clone(),
                    mempool,
                    chain_id,
                    gossip_handle,
                    storage.clone(),
                    &args_clone,
                )
            } else {
                info!(target: "aa", "Using TIPS relay mode");
                AccountAbstractionApiImpl::new(
                    ctx.provider().clone(),
                    ctx.registry.eth_api().clone(),
                    tips_url.clone(),
                    storage.clone(),
                    &args_clone,
                )
            };

            let base_aa_api = BaseAccountAbstractionApiImpl::new(
                ctx.provider().clone(),
                ctx.registry.eth_api().clone(),
            );

            // Merge RPC modules
            ctx.modules.merge_configured(aa_api.into_rpc())?;
            ctx.modules.merge_configured(base_aa_api.into_rpc())?;

            info!(
                target: "aa",
                indexer_enabled = args_clone.indexer_enabled,
                mempool_enabled = args_clone.mempool_enabled,
                debug = args_clone.debug,
                "Account Abstraction RPC enabled"
            );

            Ok(())
        })
    }
}

impl FromExtensionConfig for AccountAbstractionExtension {
    type Config = AccountAbstractionConfig;

    fn from_config(config: Self::Config) -> Self {
        Self::new(config)
    }
}

/// Start the p2p gossip service in a background task
fn start_gossip_service(
    config: GossipConfig,
    pool: Arc<RwLock<UserOpPool>>,
    chain_id: u64,
) -> Result<UserOpGossipHandle, String> {
    let cancel = CancellationToken::new();

    let (gossip, handle) =
        UserOpGossip::new(config, pool, chain_id, cancel).map_err(|e| e.to_string())?;

    // Log the multiaddresses for peer discovery
    let addrs = gossip.multiaddrs();
    info!(
        target: "aa",
        multiaddrs = ?addrs,
        "AA p2p gossip node listening"
    );

    // Spawn the gossip service in a background task
    tokio::spawn(async move {
        if let Err(e) = gossip.run().await {
            warn!(target: "aa", error = %e, "P2P gossip service error");
        }
    });

    Ok(handle)
}
