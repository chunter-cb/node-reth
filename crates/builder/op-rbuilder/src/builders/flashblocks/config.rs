use alloy_primitives::Address;

use crate::{args::OpRbuilderArgs, builders::BuilderConfig};
use core::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

/// Configuration for AA bundler integration in flashblocks builder
#[derive(Debug, Clone)]
pub struct BundlerConfig {
    /// Enable bundler loop (default: false)
    pub enabled: bool,

    /// Trigger bundling when gas used reaches this % of flashblock target
    /// Default: 30%
    pub gas_threshold_percent: u8,

    /// Maximum UserOps per bundle (per entrypoint)
    /// Default: 50
    pub max_ops_per_bundle: usize,

    /// Maximum gas per bundle (across all UserOps)
    /// Default: 21_000_000 (21M gas)
    pub max_bundle_gas: u64,

    /// Maximum retry attempts when bundle fails
    /// Each retry removes the offending UserOp and rebuilds
    /// Default: 10
    pub max_bundle_retries: u8,

    /// Beneficiary address for handleOps (receives gas refunds)
    /// Defaults to builder_signer address
    pub beneficiary: Option<Address>,
}

impl Default for BundlerConfig {
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

/// Configuration values that are specific to the flashblocks builder.
#[derive(Debug, Clone)]
pub struct FlashblocksConfig {
    /// The address of the websockets endpoint that listens for subscriptions to
    /// new flashblocks updates.
    pub ws_addr: SocketAddr,

    /// How often a flashblock is produced. This is independent of the block time of the chain.
    /// Each block will contain one or more flashblocks. On average, the number of flashblocks
    /// per block is equal to the block time divided by the flashblock interval.
    pub interval: Duration,

    /// How much time would be deducted from block build time to account for latencies in
    /// milliseconds.
    ///
    /// If dynamic_adjustment is false this value would be deducted from first flashblock and
    /// it shouldn't be more than interval
    ///
    /// If dynamic_adjustment is true this value would be deducted from first flashblock and
    /// it shouldn't be more than interval
    pub leeway_time: Duration,

    /// Disables dynamic flashblocks number adjustment based on FCU arrival time
    pub fixed: bool,

    /// Should we disable state root calculation for each flashblock
    pub disable_state_root: bool,

    /// The address of the flashblocks number contract.
    ///
    /// If set a builder tx will be added to the start of every flashblock instead of the regular builder tx.
    pub flashblocks_number_contract_address: Option<Address>,

    /// whether to use permit signatures for the contract calls
    pub flashblocks_number_contract_use_permit: bool,

    /// Whether to enable the p2p node for flashblocks
    pub p2p_enabled: bool,

    /// Port for the p2p node
    pub p2p_port: u16,

    /// Optional hex-encoded private key file path for the p2p node
    pub p2p_private_key_file: Option<String>,

    /// Comma-separated list of multiaddresses of known peers to connect to
    pub p2p_known_peers: Option<String>,

    /// Maximum number of peers for the p2p node
    pub p2p_max_peer_count: u32,

    /// AA bundler configuration
    pub bundler: BundlerConfig,
}

impl Default for FlashblocksConfig {
    fn default() -> Self {
        Self {
            ws_addr: SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 1111),
            interval: Duration::from_millis(250),
            leeway_time: Duration::from_millis(50),
            fixed: false,
            disable_state_root: false,
            flashblocks_number_contract_address: None,
            flashblocks_number_contract_use_permit: false,
            p2p_enabled: false,
            p2p_port: 9009,
            p2p_private_key_file: None,
            p2p_known_peers: None,
            p2p_max_peer_count: 50,
            bundler: BundlerConfig::default(),
        }
    }
}

impl TryFrom<OpRbuilderArgs> for FlashblocksConfig {
    type Error = eyre::Report;

    fn try_from(args: OpRbuilderArgs) -> Result<Self, Self::Error> {
        let interval = Duration::from_millis(args.flashblocks.flashblocks_block_time);

        let ws_addr = SocketAddr::new(
            args.flashblocks.flashblocks_addr.parse()?,
            args.flashblocks.flashblocks_port,
        );

        let leeway_time = Duration::from_millis(args.flashblocks.flashblocks_leeway_time);

        let fixed = args.flashblocks.flashblocks_fixed;

        let disable_state_root = args.flashblocks.flashblocks_disable_state_root;

        let flashblocks_number_contract_address =
            args.flashblocks.flashblocks_number_contract_address;

        let flashblocks_number_contract_use_permit =
            args.flashblocks.flashblocks_number_contract_use_permit;

        let bundler = BundlerConfig {
            enabled: args.flashblocks.bundler.enabled,
            gas_threshold_percent: args.flashblocks.bundler.gas_threshold_percent,
            max_ops_per_bundle: args.flashblocks.bundler.max_ops_per_bundle,
            max_bundle_gas: args.flashblocks.bundler.max_bundle_gas,
            max_bundle_retries: args.flashblocks.bundler.max_bundle_retries,
            beneficiary: args.flashblocks.bundler.beneficiary,
        };

        Ok(Self {
            ws_addr,
            interval,
            leeway_time,
            fixed,
            disable_state_root,
            flashblocks_number_contract_address,
            flashblocks_number_contract_use_permit,
            p2p_enabled: args.flashblocks.p2p.p2p_enabled,
            p2p_port: args.flashblocks.p2p.p2p_port,
            p2p_private_key_file: args.flashblocks.p2p.p2p_private_key_file,
            p2p_known_peers: args.flashblocks.p2p.p2p_known_peers,
            p2p_max_peer_count: args.flashblocks.p2p.p2p_max_peer_count,
            bundler,
        })
    }
}

pub(super) trait FlashBlocksConfigExt {
    fn flashblocks_per_block(&self) -> u64;
}

impl FlashBlocksConfigExt for BuilderConfig<FlashblocksConfig> {
    fn flashblocks_per_block(&self) -> u64 {
        if self.block_time.as_millis() == 0 {
            return 0;
        }
        (self.block_time.as_millis() / self.specific.interval.as_millis()) as u64
    }
}
