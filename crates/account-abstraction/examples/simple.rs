//! Example of using the account abstraction RPC API

use base_reth_account_abstraction::{
    AccountAbstractionApiImpl, AccountAbstractionApiServer, 
    BaseAccountAbstractionApiImpl, BaseAccountAbstractionApiServer,
};
use jsonrpsee::server::{ServerBuilder, RpcModule};
use reth_provider::test_utils::{NoopProvider, TestCanonStateSubscriptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a test provider
    let provider = NoopProvider::default();
    
    // Create account abstraction API (without gas estimation - returns hardcoded values)
    let aa_api = AccountAbstractionApiImpl::new(provider.clone());
    
    // Create base account abstraction API
    let base_aa_api = BaseAccountAbstractionApiImpl::new(provider);
    
    // Create RPC server
    let server = ServerBuilder::default().build("127.0.0.1:8546").await?;
    let mut module = RpcModule::new(());
    
    // Register both APIs
    module.merge(aa_api.into_rpc())?;
    module.merge(base_aa_api.into_rpc())?;
    
    let handle = server.start(module);
    
    println!("Account abstraction RPC server running on http://127.0.0.1:8546");
    println!("\nAvailable endpoints:");
    println!("  eth_sendUserOperation");
    println!("  eth_estimateUserOperationGas");
    println!("  eth_getUserOperationByHash"); 
    println!("  eth_getUserOperationReceipt");
    println!("  eth_supportedEntryPoints");
    println!("  base_validateUserOperation");
    println!("\nNote: This example returns hardcoded gas values.");
    println!("For real gas estimation, use AccountAbstractionApiImpl::new_with_gas_estimation()");
    println!("\nExample curl command:");
    println!("curl -X POST -H \"Content-Type: application/json\" \\");
    println!("  --data '{{\"jsonrpc\":\"2.0\",\"method\":\"eth_estimateUserOperationGas\",\"params\":[{{");
    println!("    \"sender\": \"0x0000000000000000000000000000000000000000\",");
    println!("    \"nonce\": \"0x0\",");
    println!("    \"initCode\": \"0x\",");
    println!("    \"callData\": \"0x\",");
    println!("    \"callGasLimit\": \"0x100000\",");
    println!("    \"verificationGasLimit\": \"0x100000\",");
    println!("    \"preVerificationGas\": \"0x100000\",");
    println!("    \"maxFeePerGas\": \"0x3b9aca00\",");
    println!("    \"maxPriorityFeePerGas\": \"0x3b9aca00\",");
    println!("    \"paymasterAndData\": \"0x\",");
    println!("    \"signature\": \"0x\"");
    println!("  }}, \"0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789\"],\"id\":1}}' \\");
    println!("  http://127.0.0.1:8546");
    
    // Keep server running
    handle.stopped().await;
    
    Ok(())
}