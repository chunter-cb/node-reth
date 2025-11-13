//! Gas estimation implementation using rundler
//! 
//! This integrates rundler's gas estimation logic for accurate gas calculations

use alloy_primitives::{Address, U256};
use anyhow::Result;
use std::sync::Arc;
use aa_sim::GasEstimator;
use aa_types::da::{DAGasData, DAGasBlockData};

use crate::rpc::{UserOperation, UserOperationGasEstimate, UserOperationV06, UserOperationV07};

/// A provider that can estimate gas for user operations using rundler's logic
pub struct GasEstimationProvider {
    /// RPC URL for the provider
    rpc_url: String,
}

/// Creates a gas estimation provider connected to the given RPC URL
pub fn create_gas_estimation_provider(rpc_url: &str) -> Result<GasEstimationProvider> {
    Ok(GasEstimationProvider {
        rpc_url: rpc_url.to_string(),
    })
}

impl GasEstimationProvider {
    pub async fn estimate_user_operation_gas(
        &self,
        user_op: UserOperation,
        _entry_point: Address,
    ) -> Result<UserOperationGasEstimate> {
        // Create the rundler provider and estimators
        let alloy_provider = aa_provider::new_alloy_provider(&self.rpc_url, 30)?;
        
        // Create simple default chain spec
        let chain_spec = aa_types::chain::ChainSpec {
            id: 8453,
            name: "Base".to_string(),
            entry_point_address_v0_6: "0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789".parse().unwrap(),
            entry_point_address_v0_7: "0x0000000071727De22E5E9d8BAf0edAc6f37da032".parse().unwrap(),
            ..Default::default()
        };
        
        // Create estimation settings using the public type
        let estimation_settings = aa_sim::EstimationSettings {
            max_verification_gas: 5_000_000,
            max_paymaster_verification_gas: 5_000_000,
            max_bundle_execution_gas: 10_000_000,
            max_paymaster_post_op_gas: 2_000_000,
            max_gas_estimation_gas: 20_000_000,
            verification_estimation_gas_fee: 1_000_000_000_000,
            verification_gas_limit_efficiency_reject_threshold: 0.5,
            verification_gas_allowed_error_pct: 10,
            call_gas_allowed_error_pct: 10,
            max_gas_estimation_rounds: 10,
        };
        
        // Create EVM provider
        let evm_provider = Arc::new(aa_provider::AlloyEvmProvider::new(alloy_provider.clone()));
        
        // Create DA gas oracle
        let da_gas_oracle = Arc::new(DummyDAGasOracle);
        
        // Create fee estimator
        let fee_estimator = Arc::new(aa_provider::new_fee_estimator(
            &chain_spec,
            evm_provider.clone(),
            aa_types::PriorityFeeMode::BaseFeePercent(100),
            0,
            0,
        ));
        
        // Create entry points
        let entry_point_v0_6 = Arc::new(aa_provider::AlloyEntryPointV0_6::new(
            chain_spec.clone(),
            estimation_settings.max_verification_gas.try_into().unwrap(),
            estimation_settings.max_bundle_execution_gas.try_into().unwrap(),
            estimation_settings.max_gas_estimation_gas,
            estimation_settings.max_bundle_execution_gas.try_into().unwrap(),
            alloy_provider.clone(),
            da_gas_oracle.clone(),
        ));
        
        let entry_point_v0_7 = Arc::new(aa_provider::AlloyEntryPointV0_7::new(
            chain_spec.clone(),
            estimation_settings.max_verification_gas.try_into().unwrap(),
            estimation_settings.max_bundle_execution_gas.try_into().unwrap(),
            estimation_settings.max_gas_estimation_gas,
            estimation_settings.max_bundle_execution_gas.try_into().unwrap(),
            alloy_provider,
            da_gas_oracle,
        ));
        
        // Create gas estimators  
        let gas_estimator_v0_6 = aa_sim::GasEstimatorV0_6::new(
            chain_spec.clone(),
            evm_provider.clone(),
            entry_point_v0_6,
            estimation_settings,
            fee_estimator.clone(),
        );
        
        let gas_estimator_v0_7 = aa_sim::GasEstimatorV0_7::new(
            chain_spec,
            evm_provider,
            entry_point_v0_7,
            estimation_settings,
            fee_estimator,
        );
        
        // Convert and estimate based on version
        let gas_estimate = match user_op {
            UserOperation::V06(op) => {
                let rundler_op = convert_to_rundler_v0_6(op);
                gas_estimator_v0_6
                    .estimate_op_gas(rundler_op, aa_provider::StateOverride::default())
                    .await?
            },
            UserOperation::V07(op) => {
                let rundler_op = convert_to_rundler_v0_7(op);
                gas_estimator_v0_7
                    .estimate_op_gas(rundler_op, aa_provider::StateOverride::default())
                    .await?
            },
        };
        
        // Convert back to our type
        Ok(UserOperationGasEstimate {
            pre_verification_gas: U256::from(gas_estimate.pre_verification_gas),
            verification_gas_limit: U256::from(gas_estimate.verification_gas_limit),
            call_gas_limit: U256::from(gas_estimate.call_gas_limit),
        })
    }
}

/// Convert our UserOperationV06 to rundler's format
fn convert_to_rundler_v0_6(op: UserOperationV06) -> aa_types::v0_6::UserOperationOptionalGas {
    aa_types::v0_6::UserOperationOptionalGas {
        sender: op.sender,
        nonce: op.nonce,
        init_code: op.init_code,
        call_data: op.call_data,
        call_gas_limit: Some(op.call_gas_limit.to::<u128>()),
        verification_gas_limit: Some(op.verification_gas_limit.to::<u128>()),
        pre_verification_gas: Some(op.pre_verification_gas.to::<u128>()),
        max_fee_per_gas: Some(op.max_fee_per_gas.to::<u128>()),
        max_priority_fee_per_gas: Some(op.max_priority_fee_per_gas.to::<u128>()),
        paymaster_and_data: op.paymaster_and_data,
        signature: op.signature,
        eip7702_auth_address: None,
        aggregator: None,
    }
}

/// Convert our UserOperationV07 to rundler's format
fn convert_to_rundler_v0_7(op: UserOperationV07) -> aa_types::v0_7::UserOperationOptionalGas {
    aa_types::v0_7::UserOperationOptionalGas {
        sender: op.sender,
        nonce: op.nonce,
        call_data: op.call_data,
        call_gas_limit: Some(op.call_gas_limit.to::<u128>()),
        verification_gas_limit: Some(op.verification_gas_limit.to::<u128>()),
        pre_verification_gas: Some(op.pre_verification_gas.to::<u128>()),
        max_priority_fee_per_gas: Some(op.max_priority_fee_per_gas.to::<u128>()),
        max_fee_per_gas: Some(op.max_fee_per_gas.to::<u128>()),
        signature: op.signature,
        factory: Some(op.factory),
        factory_data: op.factory_data,
        paymaster: Some(op.paymaster),
        paymaster_verification_gas_limit: Some(op.paymaster_verification_gas_limit.to::<u128>()),
        paymaster_post_op_gas_limit: Some(op.paymaster_post_op_gas_limit.to::<u128>()),
        paymaster_data: op.paymaster_data,
        eip7702_auth_address: None,
        aggregator: None,
    }
}

/// Dummy DA gas oracle for Base
struct DummyDAGasOracle;

#[async_trait::async_trait]
impl aa_provider::DAGasOracle for DummyDAGasOracle {
    async fn estimate_da_gas(
        &self,
        _bytes: alloy_primitives::Bytes,
        _to: Address,
        _block: aa_provider::BlockHashOrNumber,
        _gas_price: u128,
        _extra_data_len: usize,
    ) -> aa_provider::ProviderResult<(u128, DAGasData, DAGasBlockData)> {
        // Return dummy values for now
        Ok((0, DAGasData::default(), DAGasBlockData::default()))
    }
}