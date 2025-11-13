//! Complete RPC implementation with actual gas estimation

use alloy_primitives::Address;
use aa_provider::{StateOverride, new_alloy_provider, AlloyEvmProvider, AlloyEntryPointV0_6, AlloyEntryPointV0_7, FeeEstimator};
use aa_types::{chain::ChainSpec, UserOperationOptionalGas};
use aa_sim::{EstimationSettings, GasEstimator, GasEstimatorV0_6, GasEstimatorV0_7};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use std::sync::Arc;

use alloy_primitives::B256;

use crate::{
    rpc::{AaApiServer, BaseAaApiServer, RpcUserOperationOptionalGas, EstimateUserOperationGasResponse},
    rpc_types::{RpcUserOperationByHash, RpcUserOperationReceipt, ValidationResult},
    provider_bridge::BaseDAGasOracle,
};

/// Wrapper for FeeEstimator to make it Clone
#[derive(Clone)]
struct FeeEstimatorWrapper<F> {
    inner: Arc<F>,
}

#[async_trait::async_trait]
impl<F: FeeEstimator> FeeEstimator for FeeEstimatorWrapper<F> {
    async fn required_bundle_fees(
        &self,
        block_hash: alloy_primitives::B256,
        min_fees: Option<aa_types::GasFees>,
    ) -> anyhow::Result<(aa_types::GasFees, u128)> {
        self.inner.required_bundle_fees(block_hash, min_fees).await
    }

    async fn latest_bundle_fees(&self) -> anyhow::Result<(aa_types::GasFees, u128)> {
        self.inner.latest_bundle_fees().await
    }

    fn required_op_fees(
        &self,
        bundle_fees: aa_types::GasFees,
    ) -> aa_types::GasFees {
        self.inner.required_op_fees(bundle_fees)
    }
}

/// Full implementation that uses actual gas estimators
#[derive(Clone)]
pub struct FullAaApiImpl<G6, G7> {
    gas_estimator_v0_6: Arc<G6>,
    gas_estimator_v0_7: Arc<G7>,
}

#[async_trait]
impl<G6, G7> AaApiServer for FullAaApiImpl<G6, G7> 
where
    G6: GasEstimator<UserOperationOptionalGas = aa_types::v0_6::UserOperationOptionalGas> + Send + Sync + 'static,
    G7: GasEstimator<UserOperationOptionalGas = aa_types::v0_7::UserOperationOptionalGas> + Send + Sync + 'static,
{
    async fn send_user_operation(
        &self,
        op: RpcUserOperationOptionalGas,
        _entry_point: Address,
    ) -> RpcResult<B256> {
        // Same as simple implementation - just a stub
        tracing::info!("Received sendUserOperation request");
        match &op {
            RpcUserOperationOptionalGas::V0_6(_) => tracing::info!("Detected UserOperation v0.6"),
            RpcUserOperationOptionalGas::V0_7(_) => tracing::info!("Detected UserOperation v0.7"),
        }
        Ok(B256::random())
    }

    async fn estimate_user_operation_gas(
        &self,
        op: RpcUserOperationOptionalGas,
        _entry_point: Address,
        state_override: Option<StateOverride>,
    ) -> RpcResult<EstimateUserOperationGasResponse> {
        // Convert RPC type to domain type
        let user_op = UserOperationOptionalGas::from(op);
        
        // Determine which estimator to use based on the operation type
        let gas_estimate = match user_op {
            UserOperationOptionalGas::V0_6(op_v6) => {
                self.gas_estimator_v0_6
                    .estimate_op_gas(op_v6, state_override.unwrap_or_default())
                    .await
                    .map_err(|e| jsonrpsee::types::error::ErrorObjectOwned::owned(
                        jsonrpsee::types::error::ErrorCode::InternalError.code(),
                        format!("Gas estimation failed: {:?}", e),
                        None::<String>,
                    ))?
            },
            UserOperationOptionalGas::V0_7(op_v7) => {
                self.gas_estimator_v0_7
                    .estimate_op_gas(op_v7, state_override.unwrap_or_default())
                    .await
                    .map_err(|e| jsonrpsee::types::error::ErrorObjectOwned::owned(
                        jsonrpsee::types::error::ErrorCode::InternalError.code(),
                        format!("Gas estimation failed: {:?}", e),
                        None::<String>,
                    ))?
            },
        };
            
        Ok(EstimateUserOperationGasResponse::from(gas_estimate))
    }

    async fn get_user_operation_by_hash(
        &self,
        hash: B256,
    ) -> RpcResult<Option<RpcUserOperationByHash>> {
        // Same as simple implementation - just a stub
        tracing::info!("Received getUserOperationByHash request for hash: {:?}", hash);
        Ok(None)
    }

    async fn get_user_operation_receipt(
        &self,
        hash: B256,
    ) -> RpcResult<Option<RpcUserOperationReceipt>> {
        // Same as simple implementation - just a stub
        tracing::info!("Received getUserOperationReceipt request for hash: {:?}", hash);
        Ok(None)
    }

    async fn supported_entry_points(&self) -> RpcResult<Vec<Address>> {
        // Same as simple implementation
        Ok(vec![
            "0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789".parse().unwrap(), // v0.6
            "0x0000000071727De22E5E9d8BAf0edAc6f37da032".parse().unwrap(), // v0.7
        ])
    }
}

#[async_trait]
impl<G6, G7> BaseAaApiServer for FullAaApiImpl<G6, G7> 
where
    G6: GasEstimator<UserOperationOptionalGas = aa_types::v0_6::UserOperationOptionalGas> + Send + Sync + 'static,
    G7: GasEstimator<UserOperationOptionalGas = aa_types::v0_7::UserOperationOptionalGas> + Send + Sync + 'static,
{
    async fn validate_user_operation(
        &self,
        op: RpcUserOperationOptionalGas,
        _entry_point: Address,
    ) -> RpcResult<ValidationResult> {
        // Same as simple implementation - just a stub
        tracing::info!("Received Base validateUserOperation request");
        match &op {
            RpcUserOperationOptionalGas::V0_6(_) => tracing::info!("Validating UserOperation v0.6"),
            RpcUserOperationOptionalGas::V0_7(_) => tracing::info!("Validating UserOperation v0.7"),
        }
        Ok(ValidationResult {
            valid: true,
            reason: None,
        })
    }
}

/// Helper function to create a fully integrated API implementation from an existing alloy provider
pub fn create_full_aa_api<P>(
    alloy_provider: P,
    chain_spec: ChainSpec,
    estimation_settings: EstimationSettings,
) -> anyhow::Result<impl AaApiServer>
where
    P: alloy_provider::Provider<alloy_provider::network::AnyNetwork> + Clone + Send + Sync + 'static,
{
    // Create EVM provider by wrapping the alloy provider
    let evm_provider = Arc::new(AlloyEvmProvider::new(alloy_provider.clone()));
    
    // Create DA gas oracle
    let da_gas_oracle = Arc::new(BaseDAGasOracle);
    
    // Create fee estimator
    let fee_estimator = Arc::new(aa_provider::new_fee_estimator(
        &chain_spec,
        evm_provider.clone(),
        aa_types::PriorityFeeMode::BaseFeePercent(100),
        0,
        0,
    ));
    let fee_estimator_wrapped = Arc::new(FeeEstimatorWrapper { inner: fee_estimator });
    
    // Create entry points using the raw alloy provider
    let entry_point_v0_6 = Arc::new(AlloyEntryPointV0_6::new(
        chain_spec.clone(),
        estimation_settings.max_verification_gas.try_into().unwrap(),
        estimation_settings.max_bundle_execution_gas.try_into().unwrap(),
        estimation_settings.max_gas_estimation_gas,
        estimation_settings.max_bundle_execution_gas.try_into().unwrap(), // Using max bundle execution gas for aggregation gas
        alloy_provider.clone(),
        da_gas_oracle.clone(),
    ));
    
    let entry_point_v0_7 = Arc::new(AlloyEntryPointV0_7::new(
        chain_spec.clone(),
        estimation_settings.max_verification_gas.try_into().unwrap(),
        estimation_settings.max_bundle_execution_gas.try_into().unwrap(),
        estimation_settings.max_gas_estimation_gas,
        estimation_settings.max_bundle_execution_gas.try_into().unwrap(), // Using max bundle execution gas for aggregation gas
        alloy_provider,
        da_gas_oracle,
    ));
    
    // Create gas estimators
    let gas_estimator_v0_6 = Arc::new(GasEstimatorV0_6::new(
        chain_spec.clone(),
        evm_provider.clone(),
        entry_point_v0_6,
        estimation_settings,
        fee_estimator_wrapped.clone(),
    ));
    
    let gas_estimator_v0_7 = Arc::new(GasEstimatorV0_7::new(
        chain_spec,
        evm_provider,
        entry_point_v0_7,
        estimation_settings,
        fee_estimator_wrapped,
    ));
    
    Ok(FullAaApiImpl {
        gas_estimator_v0_6,
        gas_estimator_v0_7,
    })
}

/// Helper function to create a fully integrated API implementation using rundler's provider pattern
pub fn create_full_aa_api_from_url(
    rpc_url: &str,
) -> anyhow::Result<impl AaApiServer + BaseAaApiServer>
{
    // Create rundler's alloy provider
    let alloy_provider = new_alloy_provider(rpc_url, 30)?;
    
    // Create simple default chain spec and estimation settings
    let chain_spec = ChainSpec {
        id: 8453,
        name: "Base".to_string(),
        entry_point_address_v0_6: "0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789".parse().unwrap(),
        entry_point_address_v0_7: "0x0000000071727De22E5E9d8BAf0edAc6f37da032".parse().unwrap(),
        ..Default::default()
    };
    
    let estimation_settings = EstimationSettings {
        max_verification_gas: 5_000_000,
        max_bundle_execution_gas: 10_000_000,
        max_gas_estimation_gas: 20_000_000,
        verification_estimation_gas_fee: 1_000_000_000_000,
        verification_gas_limit_efficiency_reject_threshold: 0.5,
        verification_gas_allowed_error_pct: 10,
        call_gas_allowed_error_pct: 10,
        max_gas_estimation_rounds: 10,
    };
    
    // Use the provider-based function
    create_full_aa_api(alloy_provider, chain_spec, estimation_settings)
}