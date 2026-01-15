use eyre::Result;
use reth_optimism_rpc::OpEthApiBuilder;

use crate::{
    args::*,
    builders::{BuilderConfig, BuilderMode, FlashblocksBuilder, PayloadBuilder, StandardBuilder},
    debug_aa_rpc::{DebugBundlerApiServer, DebugBundlerRpc},
    metrics::{VERSION, record_flag_gauge_metrics},
    monitor_tx_pool::monitor_tx_pool,
    primitives::reth::engine_api_builder::OpEngineApiBuilder,
    revert_protection::{EthApiExtServer, RevertProtectionExt},
    tx::FBPooledTransaction,
    tx_data_store::{BaseApiExtServer, TxDataStoreExt},
};
use base_account_abstraction::mempool::{
    p2p::UserOpGossipHandle, GossipConfig, MempoolConfig, SharedUserOpMempoolProvider,
    UserOpGossip, UserOpPool,
};
use parking_lot::RwLock;
use tokio_util::sync::CancellationToken;
use core::fmt::Debug;
use moka::future::Cache;
use reth::builder::{NodeBuilder, WithLaunchContext};
use reth_cli_commands::launcher::Launcher;
use reth_db::mdbx::DatabaseEnv;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_cli::chainspec::OpChainSpecParser;
use reth_optimism_node::{
    OpNode,
    node::{OpAddOns, OpAddOnsBuilder, OpEngineValidatorBuilder, OpPoolBuilder},
};
use reth_transaction_pool::TransactionPool;
use std::{marker::PhantomData, sync::Arc};

pub fn launch() -> Result<()> {
    let cli = Cli::parsed();
    let mode = cli.builder_mode();

    #[cfg(feature = "telemetry")]
    let telemetry_args = match &cli.command {
        reth_optimism_cli::commands::Commands::Node(node_command) => {
            node_command.ext.telemetry.clone()
        }
        _ => Default::default(),
    };

    #[cfg(not(feature = "telemetry"))]
    let cli_app = cli.configure();

    #[cfg(feature = "telemetry")]
    let mut cli_app = cli.configure();
    #[cfg(feature = "telemetry")]
    {
        use crate::primitives::telemetry::setup_telemetry_layer;
        let telemetry_layer = setup_telemetry_layer(&telemetry_args)?;
        cli_app.access_tracing_layers()?.add_layer(telemetry_layer);
    }

    match mode {
        BuilderMode::Standard => {
            tracing::info!("Starting OP builder in standard mode");
            let launcher = BuilderLauncher::<StandardBuilder>::new();
            cli_app.run(launcher)?;
        }
        BuilderMode::Flashblocks => {
            tracing::info!("Starting OP builder in flashblocks mode");
            let launcher = BuilderLauncher::<FlashblocksBuilder>::new();
            cli_app.run(launcher)?;
        }
    }
    Ok(())
}

pub struct BuilderLauncher<B> {
    _builder: PhantomData<B>,
}

impl<B> BuilderLauncher<B>
where
    B: PayloadBuilder,
{
    pub fn new() -> Self {
        Self {
            _builder: PhantomData,
        }
    }
}

impl<B> Default for BuilderLauncher<B>
where
    B: PayloadBuilder,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<B> Launcher<OpChainSpecParser, OpRbuilderArgs> for BuilderLauncher<B>
where
    B: PayloadBuilder,
    BuilderConfig<B::Config>: TryFrom<OpRbuilderArgs>,
    <BuilderConfig<B::Config> as TryFrom<OpRbuilderArgs>>::Error: Debug,
{
    async fn entrypoint(
        self,
        builder: WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, OpChainSpec>>,
        builder_args: OpRbuilderArgs,
    ) -> Result<()> {
        let mut builder_config = BuilderConfig::<B::Config>::try_from(builder_args.clone())
            .expect("Failed to convert rollup args to builder config");

        // Create AA mempool if enabled
        // Keep a reference for the debug RPC
        let aa_pool_for_rpc: Option<(Arc<RwLock<UserOpPool>>, u64, Option<UserOpGossipHandle>)> =
            if builder_args.aa_mempool.enabled {
                let chain_id = builder.config().chain.chain().id();
                let mempool_config = MempoolConfig::default()
                    .with_max_ops_per_sender(builder_args.aa_mempool.max_ops_per_sender)
                    .with_max_pool_size(builder_args.aa_mempool.max_pool_size);

                let aa_pool = UserOpPool::new_shared(mempool_config, chain_id);
                let aa_pool_clone = aa_pool.clone();
                let aa_provider = Arc::new(SharedUserOpMempoolProvider::new(aa_pool.clone()));

                tracing::info!(
                    target: "aa",
                    max_ops_per_sender = builder_args.aa_mempool.max_ops_per_sender,
                    max_pool_size = builder_args.aa_mempool.max_pool_size,
                    chain_id = chain_id,
                    "AA mempool enabled"
                );

                // Start p2p gossip if enabled
                let gossip_handle = if builder_args.aa_mempool.p2p_enabled {
                    start_aa_gossip_service(
                        aa_pool,
                        chain_id,
                        builder_args.aa_mempool.p2p_port,
                        &builder_args.aa_mempool.p2p_peers,
                        builder_args.aa_mempool.p2p_keypair.as_deref(),
                    )
                } else {
                    None
                };

                builder_config = builder_config.with_aa_mempool(aa_provider);
                Some((aa_pool_clone, chain_id, gossip_handle))
            } else {
                None
            };

        record_flag_gauge_metrics(&builder_args);

        let da_config = builder_config.da_config.clone();
        let gas_limit_config = builder_config.gas_limit_config.clone();
        let rollup_args = builder_args.rollup_args;
        let op_node = OpNode::new(rollup_args.clone());
        let reverted_cache = Cache::builder().max_capacity(100).build();
        let reverted_cache_copy = reverted_cache.clone();
        let tx_data_store = builder_config.tx_data_store.clone();

        let mut addons: OpAddOns<
            _,
            OpEthApiBuilder,
            OpEngineValidatorBuilder,
            OpEngineApiBuilder<OpEngineValidatorBuilder>,
        > = OpAddOnsBuilder::default()
            .with_sequencer(rollup_args.sequencer.clone())
            .with_enable_tx_conditional(rollup_args.enable_tx_conditional)
            .with_da_config(da_config)
            .with_gas_limit_config(gas_limit_config)
            .build();
        if cfg!(feature = "custom-engine-api") {
            let engine_builder: OpEngineApiBuilder<OpEngineValidatorBuilder> =
                OpEngineApiBuilder::default();
            addons = addons.with_engine_api(engine_builder);
        }
        let handle = builder
            .with_types::<OpNode>()
            .with_components(
                op_node
                    .components()
                    .pool(
                        OpPoolBuilder::<FBPooledTransaction>::default()
                            .with_enable_tx_conditional(
                                // Revert protection uses the same internal pool logic as conditional transactions
                                // to garbage collect transactions out of the bundle range.
                                rollup_args.enable_tx_conditional
                                    || builder_args.enable_revert_protection,
                            )
                            .with_supervisor(
                                rollup_args.supervisor_http.clone(),
                                rollup_args.supervisor_safety_level,
                            ),
                    )
                    .payload(B::new_service(builder_config)?),
            )
            .with_add_ons(addons)
            .extend_rpc_modules(move |ctx| {
                if builder_args.enable_revert_protection {
                    tracing::info!("Revert protection enabled");

                    let pool = ctx.pool().clone();
                    let provider = ctx.provider().clone();
                    let revert_protection_ext = RevertProtectionExt::new(
                        pool,
                        provider,
                        ctx.registry.eth_api().clone(),
                        reverted_cache,
                    );

                    ctx.modules
                        .add_or_replace_configured(revert_protection_ext.into_rpc())?;
                }

                let tx_data_store_ext = TxDataStoreExt::new(tx_data_store);
                ctx.modules
                    .add_or_replace_configured(tx_data_store_ext.into_rpc())?;

                // Register debug AA RPC if mempool is enabled
                if let Some((aa_pool, chain_id, gossip_handle)) = aa_pool_for_rpc {
                    tracing::info!(
                        target: "aa",
                        p2p_enabled = gossip_handle.is_some(),
                        "Enabling debug_bundler RPC namespace (ERC-7769)"
                    );
                    let mut debug_rpc = DebugBundlerRpc::new(aa_pool, chain_id);
                    if let Some(handle) = gossip_handle {
                        debug_rpc = debug_rpc.with_gossip_handle(handle);
                    }
                    ctx.modules.add_or_replace_configured(debug_rpc.into_rpc())?;
                }

                Ok(())
            })
            .on_node_started(move |ctx| {
                VERSION.register_version_metrics();
                if builder_args.log_pool_transactions {
                    tracing::info!("Logging pool transactions");
                    let listener = ctx.pool.all_transactions_event_listener();
                    let task = monitor_tx_pool(listener, reverted_cache_copy);
                    ctx.task_executor.spawn_critical("txlogging", task);
                }
                Ok(())
            })
            .launch()
            .await?;

        handle.node_exit_future.await?;
        Ok(())
    }
}

/// Start the AA p2p gossip service and return the handle for peer management
fn start_aa_gossip_service(
    pool: Arc<RwLock<UserOpPool>>,
    chain_id: u64,
    port: u16,
    peers: &[String],
    keypair_hex: Option<&str>,
) -> Option<UserOpGossipHandle> {
    use p2p::Multiaddr;

    let known_peers: Vec<Multiaddr> = peers
        .iter()
        .filter_map(|s| {
            s.parse::<Multiaddr>()
                .map_err(|e| {
                    tracing::warn!(
                        target: "aa",
                        peer = %s,
                        error = %e,
                        "Invalid AA p2p peer address"
                    );
                    e
                })
                .ok()
        })
        .collect();

    let mut gossip_config = GossipConfig::default()
        .with_port(port)
        .with_known_peers(known_peers.clone());

    // Add keypair if provided for deterministic peer ID
    if let Some(keypair) = keypair_hex {
        gossip_config = gossip_config.with_keypair_hex(keypair.to_string());
    }

    let cancel = CancellationToken::new();

    match UserOpGossip::new(gossip_config, pool, chain_id, cancel) {
        Ok((gossip, handle)) => {
            // Log listening addresses
            let addrs = gossip.multiaddrs();
            tracing::info!(
                target: "aa",
                multiaddrs = ?addrs,
                peers = ?known_peers,
                "AA p2p gossip service starting"
            );

            // Spawn the gossip service
            tokio::spawn(async move {
                if let Err(e) = gossip.run().await {
                    tracing::warn!(target: "aa", error = %e, "AA p2p gossip service error");
                }
            });

            Some(handle)
        }
        Err(e) => {
            tracing::error!(target: "aa", error = %e, "Failed to start AA p2p gossip service");
            None
        }
    }
}
