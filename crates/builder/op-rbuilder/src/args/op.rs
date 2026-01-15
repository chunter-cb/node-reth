//! Additional Node command arguments.
//!
//! Copied from OptimismNode to allow easy extension.

//! clap [Args](clap::Args) for optimism rollup configuration

use crate::{
    flashtestations::args::FlashtestationsArgs, gas_limiter::args::GasLimiterArgs,
    tx_signer::Signer,
};
use alloy_primitives::Address;
use anyhow::{Result, anyhow};
use clap::Parser;
use reth_optimism_cli::commands::Commands;
use reth_optimism_node::args::RollupArgs;
use std::path::PathBuf;

/// Parameters for rollup configuration
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
#[command(next_help_heading = "Rollup")]
pub struct OpRbuilderArgs {
    /// Rollup configuration
    #[command(flatten)]
    pub rollup_args: RollupArgs,
    /// Builder secret key for signing last transaction in block
    #[arg(long = "rollup.builder-secret-key", env = "BUILDER_SECRET_KEY")]
    pub builder_signer: Option<Signer>,

    /// chain block time in milliseconds
    #[arg(
        long = "rollup.chain-block-time",
        default_value = "1000",
        env = "CHAIN_BLOCK_TIME"
    )]
    pub chain_block_time: u64,

    /// max gas a transaction can use
    #[arg(long = "builder.max_gas_per_txn")]
    pub max_gas_per_txn: Option<u64>,

    /// Signals whether to log pool transaction events
    #[arg(long = "builder.log-pool-transactions", default_value = "false")]
    pub log_pool_transactions: bool,

    /// How much time extra to wait for the block building job to complete and not get garbage collected
    #[arg(long = "builder.extra-block-deadline-secs", default_value = "20")]
    pub extra_block_deadline_secs: u64,
    /// Whether to enable revert protection by default
    #[arg(long = "builder.enable-revert-protection", default_value = "false")]
    pub enable_revert_protection: bool,
    /// Whether to enable TIPS Resource Metering
    #[arg(long = "builder.enable-resource-metering", default_value = "false")]
    pub enable_resource_metering: bool,

    /// Buffer size for tx data store (LRU eviction when full)
    #[arg(long = "builder.tx-data-store-buffer-size", default_value = "10000")]
    pub tx_data_store_buffer_size: usize,

    /// Path to builder playgorund to automatically start up the node connected to it
    #[arg(
        long = "builder.playground",
        num_args = 0..=1,
        default_missing_value = "$HOME/.playground/devnet/",
        value_parser = expand_path,
        env = "PLAYGROUND_DIR",
    )]
    pub playground: Option<PathBuf>,
    #[command(flatten)]
    pub flashblocks: FlashblocksArgs,
    #[command(flatten)]
    pub telemetry: TelemetryArgs,
    #[command(flatten)]
    pub flashtestations: FlashtestationsArgs,
    #[command(flatten)]
    pub gas_limiter: GasLimiterArgs,
    #[command(flatten)]
    pub aa_mempool: AAMempoolArgs,
}

impl Default for OpRbuilderArgs {
    fn default() -> Self {
        let args = crate::args::Cli::parse_from(["dummy", "node"]);
        let Commands::Node(node_command) = args.command else {
            unreachable!()
        };
        node_command.ext
    }
}

fn expand_path(s: &str) -> Result<PathBuf> {
    shellexpand::full(s)
        .map_err(|e| anyhow!("expansion error for `{s}`: {e}"))?
        .into_owned()
        .parse()
        .map_err(|e| anyhow!("invalid path after expansion: {e}"))
}

/// Parameters for Flashblocks configuration
/// The names in the struct are prefixed with `flashblocks` to avoid conflicts
/// with the standard block building configuration since these args are flattened
/// into the main `OpRbuilderArgs` struct with the other rollup/node args.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct FlashblocksArgs {
    /// When set to true, the builder will build flashblocks
    /// and will build standard blocks at the chain block time.
    ///
    /// The default value will change in the future once the flashblocks
    /// feature is stable.
    #[arg(
        long = "flashblocks.enabled",
        default_value = "false",
        env = "ENABLE_FLASHBLOCKS"
    )]
    pub enabled: bool,

    /// The port that we bind to for the websocket server that provides flashblocks
    #[arg(
        long = "flashblocks.port",
        env = "FLASHBLOCKS_WS_PORT",
        default_value = "1111"
    )]
    pub flashblocks_port: u16,

    /// The address that we bind to for the websocket server that provides flashblocks
    #[arg(
        long = "flashblocks.addr",
        env = "FLASHBLOCKS_WS_ADDR",
        default_value = "127.0.0.1"
    )]
    pub flashblocks_addr: String,

    /// flashblock block time in milliseconds
    #[arg(
        long = "flashblocks.block-time",
        default_value = "250",
        env = "FLASHBLOCK_BLOCK_TIME"
    )]
    pub flashblocks_block_time: u64,

    /// Builder would always thry to produce fixed number of flashblocks without regard to time of
    /// FCU arrival.
    /// In cases of late FCU it could lead to partially filled blocks.
    #[arg(
        long = "flashblocks.fixed",
        default_value = "false",
        env = "FLASHBLOCK_FIXED"
    )]
    pub flashblocks_fixed: bool,

    /// Time by which blocks would be completed earlier in milliseconds.
    ///
    /// This time used to account for latencies, this time would be deducted from total block
    /// building time before calculating number of fbs.
    #[arg(
        long = "flashblocks.leeway-time",
        default_value = "75",
        env = "FLASHBLOCK_LEEWAY_TIME"
    )]
    pub flashblocks_leeway_time: u64,

    /// Whether to disable state root calculation for each flashblock
    #[arg(
        long = "flashblocks.disable-state-root",
        default_value = "false",
        env = "FLASHBLOCKS_DISABLE_STATE_ROOT"
    )]
    pub flashblocks_disable_state_root: bool,

    /// Flashblocks number contract address
    ///
    /// This is the address of the contract that will be used to increment the flashblock number.
    /// If set a builder tx will be added to the start of every flashblock instead of the regular builder tx.
    #[arg(
        long = "flashblocks.number-contract-address",
        env = "FLASHBLOCK_NUMBER_CONTRACT_ADDRESS"
    )]
    pub flashblocks_number_contract_address: Option<Address>,

    /// Use permit signatures if flashtestations is enabled with the flashtestation key
    /// to increment the flashblocks number
    #[arg(
        long = "flashblocks.number-contract-use-permit",
        env = "FLASHBLOCK_NUMBER_CONTRACT_USE_PERMIT",
        default_value = "false"
    )]
    pub flashblocks_number_contract_use_permit: bool,

    /// Flashblocks p2p configuration
    #[command(flatten)]
    pub p2p: FlashblocksP2pArgs,

    /// AA Bundler configuration
    #[command(flatten)]
    pub bundler: BundlerArgs,
}

impl Default for FlashblocksArgs {
    fn default() -> Self {
        let args = crate::args::Cli::parse_from(["dummy", "node"]);
        let Commands::Node(node_command) = args.command else {
            unreachable!()
        };
        node_command.ext.flashblocks
    }
}

#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct FlashblocksP2pArgs {
    /// Enable libp2p networking for flashblock propagation
    #[arg(
        long = "flashblocks.p2p_enabled",
        env = "FLASHBLOCK_P2P_ENABLED",
        default_value = "false"
    )]
    pub p2p_enabled: bool,

    /// Port for the flashblocks p2p node
    #[arg(
        long = "flashblocks.p2p_port",
        env = "FLASHBLOCK_P2P_PORT",
        default_value = "9009"
    )]
    pub p2p_port: u16,

    /// Path to the file containing a hex-encoded libp2p private key.
    /// If the file does not exist, a new key will be generated.
    #[arg(
        long = "flashblocks.p2p_private_key_file",
        env = "FLASHBLOCK_P2P_PRIVATE_KEY_FILE"
    )]
    pub p2p_private_key_file: Option<String>,

    /// Comma-separated list of multiaddrs of known Flashblocks peers
    /// Example: "/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ,/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ"
    #[arg(
        long = "flashblocks.p2p_known_peers",
        env = "FLASHBLOCK_P2P_KNOWN_PEERS"
    )]
    pub p2p_known_peers: Option<String>,

    /// Maximum number of peers for the flashblocks p2p node
    #[arg(
        long = "flashblocks.p2p_max_peer_count",
        env = "FLASHBLOCK_P2P_MAX_PEER_COUNT",
        default_value = "50"
    )]
    pub p2p_max_peer_count: u32,
}

/// Parameters for AA Bundler configuration
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct BundlerArgs {
    /// Enable AA bundler in flashblocks builder
    #[arg(
        long = "flashblocks.bundler-enabled",
        env = "FLASHBLOCKS_BUNDLER_ENABLED",
        default_value = "false",
        id = "bundler_enabled"
    )]
    pub enabled: bool,

    /// Gas percentage threshold to trigger bundling (0-100)
    /// Bundling starts when standard tx gas usage reaches this % of flashblock target
    #[arg(
        long = "flashblocks.bundler-gas-threshold",
        env = "FLASHBLOCKS_BUNDLER_GAS_THRESHOLD",
        default_value = "30",
        id = "bundler_gas_threshold"
    )]
    pub gas_threshold_percent: u8,

    /// Maximum UserOps per bundle (per entrypoint)
    #[arg(
        long = "flashblocks.bundler-max-ops",
        env = "FLASHBLOCKS_BUNDLER_MAX_OPS",
        default_value = "50",
        id = "bundler_max_ops"
    )]
    pub max_ops_per_bundle: usize,

    /// Maximum gas per bundle (across all UserOps)
    #[arg(
        long = "flashblocks.bundler-max-gas",
        env = "FLASHBLOCKS_BUNDLER_MAX_GAS",
        default_value = "21000000",
        id = "bundler_max_gas"
    )]
    pub max_bundle_gas: u64,

    /// Maximum bundle build retries when UserOps fail
    /// Each retry removes the offending UserOp and rebuilds
    #[arg(
        long = "flashblocks.bundler-max-retries",
        env = "FLASHBLOCKS_BUNDLER_MAX_RETRIES",
        default_value = "10",
        id = "bundler_max_retries"
    )]
    pub max_bundle_retries: u8,

    /// Beneficiary address for handleOps (receives gas refunds)
    /// Defaults to builder_signer address if not set
    #[arg(
        long = "flashblocks.bundler-beneficiary",
        env = "FLASHBLOCKS_BUNDLER_BENEFICIARY",
        id = "bundler_beneficiary"
    )]
    pub beneficiary: Option<Address>,
}

impl Default for BundlerArgs {
    fn default() -> Self {
        Self {
            enabled: false,
            gas_threshold_percent: 30,
            max_ops_per_bundle: 50,
            max_bundle_gas: 21_000_000,
            max_bundle_retries: 10,
            beneficiary: None,
        }
    }
}

/// Parameters for Account Abstraction mempool configuration
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct AAMempoolArgs {
    /// Enable the AA (ERC-4337) UserOperation mempool.
    /// When enabled, the builder will create and manage a local mempool for UserOperations.
    #[arg(
        long = "aa.mempool-enabled",
        env = "AA_MEMPOOL_ENABLED",
        default_value = "false",
        id = "aa_mempool_enabled"
    )]
    pub enabled: bool,

    /// Maximum UserOps per sender in the mempool
    #[arg(
        long = "aa.max-ops-per-sender",
        env = "AA_MAX_OPS_PER_SENDER",
        default_value = "4",
        id = "aa_max_ops_per_sender"
    )]
    pub max_ops_per_sender: usize,

    /// Maximum total UserOps per entrypoint in the mempool
    #[arg(
        long = "aa.max-pool-size",
        env = "AA_MAX_POOL_SIZE",
        default_value = "10000",
        id = "aa_max_pool_size"
    )]
    pub max_pool_size: usize,

    /// Enable p2p gossip for AA UserOperations.
    /// When enabled, UserOps will be shared with connected peers.
    #[arg(
        long = "aa.p2p-enabled",
        env = "AA_P2P_ENABLED",
        default_value = "false",
        id = "aa_p2p_enabled"
    )]
    pub p2p_enabled: bool,

    /// Port for the AA p2p gossip service
    #[arg(
        long = "aa.p2p-port",
        env = "AA_P2P_PORT",
        default_value = "9546",
        id = "aa_p2p_port"
    )]
    pub p2p_port: u16,

    /// Known AA p2p peers to connect to (comma-separated multiaddrs)
    /// Example: /ip4/127.0.0.1/tcp/9545/p2p/12D3KooW...
    #[arg(
        long = "aa.p2p-peers",
        env = "AA_P2P_PEERS",
        value_delimiter = ',',
        id = "aa_p2p_peers"
    )]
    pub p2p_peers: Vec<String>,

    /// Hex-encoded ed25519 private key for p2p node identity.
    /// If not provided, a random key is generated on each startup.
    /// This allows deterministic peer IDs for easier configuration.
    #[arg(
        long = "aa.p2p-keypair",
        env = "AA_P2P_KEYPAIR",
        id = "aa_p2p_keypair"
    )]
    pub p2p_keypair: Option<String>,
}

impl Default for AAMempoolArgs {
    fn default() -> Self {
        Self {
            enabled: false,
            max_ops_per_sender: 4,
            max_pool_size: 10_000,
            p2p_enabled: false,
            p2p_port: 9546,
            p2p_peers: Vec::new(),
            p2p_keypair: None,
        }
    }
}

/// Parameters for telemetry configuration
#[derive(Debug, Clone, Default, PartialEq, Eq, clap::Args)]
pub struct TelemetryArgs {
    /// OpenTelemetry endpoint for traces
    #[arg(long = "telemetry.otlp-endpoint", env = "OTEL_EXPORTER_OTLP_ENDPOINT")]
    pub otlp_endpoint: Option<String>,

    /// OpenTelemetry headers for authentication
    #[arg(long = "telemetry.otlp-headers", env = "OTEL_EXPORTER_OTLP_HEADERS")]
    pub otlp_headers: Option<String>,

    /// Inverted sampling frequency in blocks. 1 - each block, 100 - every 100th block.
    #[arg(
        long = "telemetry.sampling-ratio",
        env = "SAMPLING_RATIO",
        default_value = "100"
    )]
    pub sampling_ratio: u64,
}
