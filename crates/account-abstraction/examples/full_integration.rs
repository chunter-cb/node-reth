//! Example of fully integrated account abstraction gas estimation

use base_reth_account_abstraction::{
    default_base_mainnet_spec, default_estimation_settings,
    AaApiServer, create_full_aa_api_from_url,
};
use jsonrpsee::server::{ServerBuilder, RpcModule};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Configure chain spec and settings
    let chain_spec = default_base_mainnet_spec();
    let estimation_settings = default_estimation_settings();
    
    // Get RPC URL from environment or use default
    let rpc_url = std::env::var("ETH_RPC_URL")
        .unwrap_or_else(|_| "http://localhost:8545".to_string());
    
    println!("Creating fully integrated account abstraction API...");
    println!("Connecting to: {}", rpc_url);
    
    // Create the fully integrated account abstraction API with actual gas estimation
    let aa_api = create_full_aa_api_from_url(
        &rpc_url,
        chain_spec.clone(),
        estimation_settings,
    )?;
    
    println!();
    println!("Chain spec: Base mainnet (chain ID: {})", chain_spec.id);
    println!("EntryPoint v0.6: {}", chain_spec.entry_point_address_v0_6);
    println!("EntryPoint v0.7: {}", chain_spec.entry_point_address_v0_7);
    println!();
    println!("Gas estimation settings:");
    println!("  Max verification gas: {}", estimation_settings.max_verification_gas);
    println!("  Max bundle execution gas: {}", estimation_settings.max_bundle_execution_gas);
    println!("  Max gas estimation rounds: {}", estimation_settings.max_gas_estimation_rounds);
    println!();
    
    // Create RPC server
    let server = ServerBuilder::default().build("127.0.0.1:8546").await?;
    let mut module = RpcModule::new(());
    
    // Register the account abstraction API
    module.merge(aa_api.into_rpc())?;
    
    let handle = server.start(module);
    
    println!("Account abstraction RPC server running on http://127.0.0.1:8546");
    println!("Gas estimation endpoint: eth_estimateUserOperationGas");
    println!();
    println!("✅ FULL RUNDLER GAS ESTIMATION INTEGRATED!");
    println!("This server now performs actual gas estimation using rundler's complete logic.");
    println!();
    println!("Features integrated:");
    println!("  - Binary search gas estimation for call gas");
    println!("  - Verification gas estimation with state overrides");
    println!("  - Pre-verification gas calculation");
    println!("  - Support for both EntryPoint v0.6 and v0.7");
    println!("  - DA gas estimation for L2s");
    println!();
    println!("Example v0.6 UserOperation curl command:");
    println!("curl -X POST -H \"Content-Type: application/json\" \\");
    println!("  --data '{{\"jsonrpc\":\"2.0\",\"method\":\"eth_estimateUserOperationGas\",\"params\":[{{");
    println!("    \"sender\": \"0x0000000000000000000000000000000000000000\",");
    println!("    \"nonce\": \"0x0\",");
    println!("    \"initCode\": \"0x\",");
    println!("    \"callData\": \"0x\",");
    println!("    \"paymasterAndData\": \"0x\",");
    println!("    \"signature\": \"0x\"");
    println!("  }}, \"0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789\", null],\"id\":1}}' \\");
    println!("  http://127.0.0.1:8546");
    
    // Keep server running
    handle.stopped().await;
    
    Ok(())
}