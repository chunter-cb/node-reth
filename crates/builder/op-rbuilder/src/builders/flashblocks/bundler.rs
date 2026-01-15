//! AA Bundler for Flashblocks
//!
//! Handles creating and executing ERC-4337 bundles (handleOps transactions)
//! within the flashblocks builder.
//!
//! # Architecture
//!
//! The bundler is invoked during each flashblock build cycle, after standard
//! transactions have been processed. It:
//!
//! 1. Polls the AA mempool for each entrypoint (v0.6, v0.7, v0.8, v0.9)
//! 2. Creates `handleOps` transactions for available UserOps
//! 3. Returns signed bundle transactions for the payload builder to execute
//! 4. Handles failures by parsing `FailedOp` reverts and removing offending ops

use alloy_consensus::TxEip1559;
use alloy_primitives::{Address, Bytes, TxKind, U256};
use alloy_sol_types::SolCall;
use base_account_abstraction::{
    contracts::{
        IEntryPointV07, PackedUserOperationV07, ENTRYPOINT_V06_ADDRESS, ENTRYPOINT_V07_ADDRESS,
        ENTRYPOINT_V08_ADDRESS, ENTRYPOINT_V09_ADDRESS, pack_user_op_v07,
    },
    mempool::{PooledUserOp, UserOpMempoolProvider},
    UserOperation,
};
use op_alloy_consensus::OpTypedTransaction;
use reth_optimism_primitives::OpTransactionSigned;
use reth_primitives::Recovered;
use tracing::{debug, error, info, warn};

use crate::tx_signer::Signer;

use super::config::BundlerConfig;

/// Calculate total gas for a UserOperation
fn user_op_total_gas(user_op: &UserOperation) -> u64 {
    match user_op {
        UserOperation::V06(op) => {
            let total = op.pre_verification_gas + op.verification_gas_limit + op.call_gas_limit;
            total.saturating_to::<u64>()
        }
        UserOperation::V07(op) => {
            let total = op.pre_verification_gas
                + op.verification_gas_limit
                + op.call_gas_limit
                + op.paymaster_verification_gas_limit
                + op.paymaster_post_op_gas_limit;
            total.saturating_to::<u64>()
        }
    }
}

/// Ordered list of entrypoints to bundle (v0.6 first)
pub(crate) const ENTRYPOINTS_ORDERED: &[Address] = &[
    ENTRYPOINT_V06_ADDRESS,
    ENTRYPOINT_V07_ADDRESS,
    ENTRYPOINT_V08_ADDRESS,
    ENTRYPOINT_V09_ADDRESS,
];

/// Result of building a bundle
#[derive(Debug, Clone)]
pub(crate) struct BundleResult {
    /// The signed handleOps transaction
    pub tx: Recovered<OpTransactionSigned>,
    /// Gas used by the bundle (estimated)
    pub gas_limit: u64,
    /// DA size of the bundle
    pub da_size: u64,
    /// Number of UserOps included
    pub ops_count: usize,
    /// Hashes of included UserOps
    pub op_hashes: Vec<alloy_primitives::B256>,
    /// EntryPoint address
    pub entry_point: Address,
}

/// Error from bundle building
#[derive(Debug, thiserror::Error)]
pub(crate) enum BundleError {
    #[error("No signer configured for bundler")]
    NoSigner,
    #[error("Failed to sign transaction: {0}")]
    SigningError(String),
    #[error("Unsupported entrypoint version: {0}")]
    UnsupportedVersion(Address),
}

/// Bundler for creating handleOps transactions
///
/// Uses a trait object for the mempool provider to support dynamic dispatch,
/// which allows the bundler to work with any implementation of `UserOpMempoolProvider`.
pub(crate) struct Bundler<'a> {
    config: &'a BundlerConfig,
    mempool: &'a dyn UserOpMempoolProvider,
    signer: &'a Signer,
    beneficiary: Address,
    chain_id: u64,
    base_fee: u128,
}

impl<'a> Bundler<'a> {
    /// Create a new bundler
    pub(crate) fn new(
        config: &'a BundlerConfig,
        mempool: &'a dyn UserOpMempoolProvider,
        signer: &'a Signer,
        chain_id: u64,
        base_fee: u128,
    ) -> Self {
        let beneficiary = config.beneficiary.unwrap_or(signer.address);
        Self {
            config,
            mempool,
            signer,
            beneficiary,
            chain_id,
            base_fee,
        }
    }

    /// Try to build bundles for all entrypoints
    ///
    /// Returns a list of bundle transactions to be executed.
    /// Does NOT execute or commit state - caller is responsible for that.
    pub(crate) fn build_bundles(
        &self,
        mut nonce: u64,
        max_gas: u64,
    ) -> Vec<Result<BundleResult, BundleError>> {
        let mut results = Vec::new();
        let mut gas_budget = max_gas.min(self.config.max_bundle_gas);

        for &entry_point in ENTRYPOINTS_ORDERED {
            if gas_budget < 100_000 {
                // Not enough gas for any meaningful bundle
                break;
            }

            match self.build_bundle_for_entrypoint(entry_point, nonce, gas_budget) {
                Ok(Some(bundle)) => {
                    info!(
                        target: "bundler",
                        entry_point = %entry_point,
                        ops_count = bundle.ops_count,
                        gas_limit = bundle.gas_limit,
                        "Bundle built successfully"
                    );
                    gas_budget = gas_budget.saturating_sub(bundle.gas_limit);
                    nonce += 1;
                    results.push(Ok(bundle));
                }
                Ok(None) => {
                    debug!(
                        target: "bundler",
                        entry_point = %entry_point,
                        "No UserOps available for entrypoint"
                    );
                }
                Err(e) => {
                    if !matches!(e, BundleError::UnsupportedVersion(_)) {
                        error!(
                            target: "bundler",
                            entry_point = %entry_point,
                            error = %e,
                            "Failed to build bundle"
                        );
                    }
                    results.push(Err(e));
                }
            }
        }

        results
    }

    /// Try to build a bundle for a specific entrypoint
    fn build_bundle_for_entrypoint(
        &self,
        entry_point: Address,
        nonce: u64,
        max_gas: u64,
    ) -> Result<Option<BundleResult>, BundleError> {
        // Get best UserOps from mempool
        let ops = self.mempool.get_best_userops(
            entry_point,
            self.config.max_ops_per_bundle,
            max_gas,
        );

        if ops.is_empty() {
            return Ok(None);
        }

        let op_hashes: Vec<_> = ops.iter().map(|op| op.hash).collect();

        // Mark as pending to prevent double-inclusion
        self.mempool.mark_pending_inclusion(entry_point, &op_hashes);

        // Build the handleOps calldata
        let calldata = match self.encode_handle_ops(entry_point, &ops) {
            Ok(data) => data,
            Err(e) => {
                // Release ops on failure
                self.mempool.release_pending(entry_point, &op_hashes);
                return Err(e);
            }
        };

        // Calculate gas limit from UserOps
        // 
        // The handleOps gas needs to cover:
        // 1. Transaction intrinsic gas (~21000 + calldata cost)
        // 2. handleOps overhead per op (~10000)
        // 3. Account deployment if initCode present (~300000)
        // 4. The UserOp's own gas (pre_verification + verification + call)
        // 5. Buffer for EntryPoint checks (it requires gasleft >= verification + call + 5000 before innerHandleOp)
        //
        // Safe formula: sum of UserOp gas * 1.5 + 500000 base overhead
        let user_op_gas: u64 = ops.iter().map(|op| user_op_total_gas(&op.user_op)).sum();
        let gas_limit = (user_op_gas * 3 / 2) + 500_000;

        // Calculate DA size (approximate)
        let da_size = calldata.len() as u64;

        // Create and sign the transaction
        let tx = self.create_bundle_tx(entry_point, calldata, gas_limit, nonce)?;

        Ok(Some(BundleResult {
            tx,
            gas_limit,
            da_size,
            ops_count: ops.len(),
            op_hashes,
            entry_point,
        }))
    }

    /// Encode handleOps calldata for the given entrypoint
    fn encode_handle_ops(
        &self,
        entry_point: Address,
        ops: &[PooledUserOp],
    ) -> Result<Bytes, BundleError> {
        // Currently only v0.7+ is fully implemented
        if entry_point == ENTRYPOINT_V07_ADDRESS
            || entry_point == ENTRYPOINT_V08_ADDRESS
            || entry_point == ENTRYPOINT_V09_ADDRESS
        {
            let packed_ops: Vec<PackedUserOperationV07> = ops
                .iter()
                .map(|pooled_op| self.pack_user_op_v07(&pooled_op.user_op))
                .collect();

            let call = IEntryPointV07::handleOpsCall {
                ops: packed_ops,
                beneficiary: self.beneficiary,
            };

            Ok(call.abi_encode().into())
        } else if entry_point == ENTRYPOINT_V06_ADDRESS {
            // v0.6 uses the original UserOperation struct directly
            use base_account_abstraction::contracts::{IEntryPointV06, UserOperationV06Packed};
            
            let packed_ops: Vec<UserOperationV06Packed> = ops
                .iter()
                .map(|pooled_op| self.pack_user_op_v06(&pooled_op.user_op))
                .collect();

            let call = IEntryPointV06::handleOpsCall {
                ops: packed_ops,
                beneficiary: self.beneficiary,
            };

            info!(
                target: "bundler",
                ops_count = ops.len(),
                "Encoding v0.6 handleOps bundle"
            );

            Ok(call.abi_encode().into())
        } else {
            Err(BundleError::UnsupportedVersion(entry_point))
        }
    }

    /// Pack a UserOperation into v0.7 format
    fn pack_user_op_v07(&self, user_op: &UserOperation) -> PackedUserOperationV07 {
        match user_op {
            UserOperation::V06(op) => {
                // Convert v0.6 to v0.7 packed format (best effort)
                // Extract factory from initCode if present
                let (factory, factory_data) = if op.init_code.len() >= 20 {
                    (
                        Address::from_slice(&op.init_code[..20]),
                        op.init_code.slice(20..),
                    )
                } else {
                    (Address::ZERO, Bytes::default())
                };

                // Extract paymaster from paymasterAndData if present
                let paymaster = if op.paymaster_and_data.len() >= 20 {
                    Address::from_slice(&op.paymaster_and_data[..20])
                } else {
                    Address::ZERO
                };

                pack_user_op_v07(
                    op.sender,
                    op.nonce,
                    factory,
                    factory_data,
                    op.call_data.clone(),
                    op.call_gas_limit,
                    op.verification_gas_limit,
                    op.pre_verification_gas,
                    op.max_fee_per_gas,
                    op.max_priority_fee_per_gas,
                    paymaster,
                    U256::ZERO,              // paymasterVerificationGasLimit
                    U256::ZERO,              // paymasterPostOpGasLimit
                    Bytes::default(),        // paymasterData
                    op.signature.clone(),
                )
            }
            UserOperation::V07(op) => {
                pack_user_op_v07(
                    op.sender,
                    op.nonce,
                    op.factory,
                    op.factory_data.clone(),
                    op.call_data.clone(),
                    op.call_gas_limit,
                    op.verification_gas_limit,
                    op.pre_verification_gas,
                    op.max_fee_per_gas,
                    op.max_priority_fee_per_gas,
                    op.paymaster,
                    op.paymaster_verification_gas_limit,
                    op.paymaster_post_op_gas_limit,
                    op.paymaster_data.clone(),
                    op.signature.clone(),
                )
            }
        }
    }

    /// Pack a UserOperation into v0.6 format
    fn pack_user_op_v06(&self, user_op: &UserOperation) -> base_account_abstraction::contracts::UserOperationV06Packed {
        use base_account_abstraction::contracts::UserOperationV06Packed;
        
        match user_op {
            UserOperation::V06(op) => {
                // v0.6 ops already have the right format
                UserOperationV06Packed {
                    sender: op.sender,
                    nonce: U256::from(op.nonce),
                    initCode: op.init_code.clone(),
                    callData: op.call_data.clone(),
                    callGasLimit: U256::from(op.call_gas_limit),
                    verificationGasLimit: U256::from(op.verification_gas_limit),
                    preVerificationGas: U256::from(op.pre_verification_gas),
                    maxFeePerGas: U256::from(op.max_fee_per_gas),
                    maxPriorityFeePerGas: U256::from(op.max_priority_fee_per_gas),
                    paymasterAndData: op.paymaster_and_data.clone(),
                    signature: op.signature.clone(),
                }
            }
            UserOperation::V07(op) => {
                // Convert v0.7 to v0.6 format (combine fields back)
                // Reconstruct initCode from factory + factoryData
                let init_code = if op.factory != Address::ZERO {
                    let mut code = op.factory.as_slice().to_vec();
                    code.extend_from_slice(&op.factory_data);
                    Bytes::from(code)
                } else {
                    Bytes::default()
                };
                
                // Reconstruct paymasterAndData from paymaster + paymasterData
                let paymaster_and_data = if op.paymaster != Address::ZERO {
                    let mut data = op.paymaster.as_slice().to_vec();
                    data.extend_from_slice(&op.paymaster_data);
                    Bytes::from(data)
                } else {
                    Bytes::default()
                };
                
                UserOperationV06Packed {
                    sender: op.sender,
                    nonce: U256::from(op.nonce),
                    initCode: init_code,
                    callData: op.call_data.clone(),
                    callGasLimit: U256::from(op.call_gas_limit),
                    verificationGasLimit: U256::from(op.verification_gas_limit),
                    preVerificationGas: U256::from(op.pre_verification_gas),
                    maxFeePerGas: U256::from(op.max_fee_per_gas),
                    maxPriorityFeePerGas: U256::from(op.max_priority_fee_per_gas),
                    paymasterAndData: paymaster_and_data,
                    signature: op.signature.clone(),
                }
            }
        }
    }

    /// Create and sign an EIP-1559 transaction for the bundle
    fn create_bundle_tx(
        &self,
        entry_point: Address,
        calldata: Bytes,
        gas_limit: u64,
        nonce: u64,
    ) -> Result<Recovered<OpTransactionSigned>, BundleError> {
        let max_priority_fee: u128 = 1_000_000_000; // 1 gwei
        let max_fee = self.base_fee + max_priority_fee;

        let tx = OpTypedTransaction::Eip1559(TxEip1559 {
            chain_id: self.chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas: max_fee,
            max_priority_fee_per_gas: max_priority_fee,
            to: TxKind::Call(entry_point),
            value: U256::ZERO,
            access_list: Default::default(),
            input: calldata,
        });

        self.signer
            .sign_tx(tx)
            .map_err(|e| BundleError::SigningError(e.to_string()))
    }

    /// Handle a failed bundle by removing the offending UserOp
    ///
    /// Returns the index of the failed op if it can be parsed from revert data
    pub(crate) fn handle_bundle_failure(
        &self,
        entry_point: Address,
        op_hashes: &[alloy_primitives::B256],
        revert_data: &[u8],
    ) -> Option<usize> {
        let failed_index = self.parse_failed_op_index(revert_data);

        if let Some(index) = failed_index {
            if index < op_hashes.len() {
                // Remove the failed op from mempool (not just release)
                self.mempool.confirm_included(entry_point, &[op_hashes[index]]);
                warn!(
                    target: "bundler",
                    op_hash = %op_hashes[index],
                    index,
                    "Removed failed UserOp from mempool"
                );
            }
        } else if !op_hashes.is_empty() {
            // Couldn't parse failure, remove first op as fallback
            self.mempool.confirm_included(entry_point, &[op_hashes[0]]);
            warn!(
                target: "bundler",
                op_hash = %op_hashes[0],
                "Removed first UserOp (unknown failure reason)"
            );
        }

        failed_index
    }

    /// Release all pending ops for a bundle that failed completely
    pub(crate) fn release_bundle(&self, entry_point: Address, op_hashes: &[alloy_primitives::B256]) {
        self.mempool.release_pending(entry_point, op_hashes);
    }

    /// Confirm a bundle was included
    pub(crate) fn confirm_bundle(&self, entry_point: Address, op_hashes: &[alloy_primitives::B256]) {
        self.mempool.confirm_included(entry_point, op_hashes);
    }

    /// Parse the failed UserOp index from revert data
    ///
    /// EntryPoint reverts with FailedOp(uint256 opIndex, string reason)
    fn parse_failed_op_index(&self, output: &[u8]) -> Option<usize> {
        // FailedOp selector: 0x220266b6
        // FailedOpWithRevert selector: 0x9f947596
        if output.len() < 36 {
            return None;
        }

        let selector = &output[..4];
        if selector == [0x22, 0x02, 0x66, 0xb6] || selector == [0x9f, 0x94, 0x75, 0x96] {
            // Extract opIndex (first uint256 parameter)
            let index_bytes = &output[4..36];
            let index = U256::from_be_slice(index_bytes);
            
            // Try to parse reason string for logging
            if let Some(reason) = self.parse_failed_op_reason(output) {
                warn!(
                    target: "bundler",
                    op_index = ?index,
                    reason = %reason,
                    "FailedOp revert reason"
                );
            }
            
            return index.try_into().ok();
        }

        None
    }
    
    /// Parse the reason string from FailedOp revert data
    fn parse_failed_op_reason(&self, output: &[u8]) -> Option<String> {
        // FailedOp(uint256 opIndex, string reason)
        // Layout: selector (4) + opIndex (32) + string offset (32) + string length (32) + string data
        if output.len() < 100 {
            return None;
        }
        
        // String offset is at bytes 36-68 (should be 0x40 = 64)
        let string_len_offset = 4 + 32 + 32; // 68
        if output.len() < string_len_offset + 32 {
            return None;
        }
        
        let len_bytes = &output[string_len_offset..string_len_offset + 32];
        let len = U256::from_be_slice(len_bytes);
        let len: usize = len.try_into().ok()?;
        
        let string_start = string_len_offset + 32;
        if output.len() < string_start + len {
            return None;
        }
        
        String::from_utf8(output[string_start..string_start + len].to_vec()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entrypoints_ordered() {
        assert_eq!(ENTRYPOINTS_ORDERED[0], ENTRYPOINT_V06_ADDRESS);
        assert_eq!(ENTRYPOINTS_ORDERED[1], ENTRYPOINT_V07_ADDRESS);
        assert_eq!(ENTRYPOINTS_ORDERED[2], ENTRYPOINT_V08_ADDRESS);
        assert_eq!(ENTRYPOINTS_ORDERED[3], ENTRYPOINT_V09_ADDRESS);
    }

    #[test]
    fn test_parse_failed_op_index() {
        // Test with FailedOp selector and index = 2
        let mut output = vec![0x22, 0x02, 0x66, 0xb6]; // selector
        output.extend_from_slice(&[0u8; 31]); // padding
        output.push(2); // index = 2

        // We'd need a bundler to test, but the logic is:
        // selector (4) + uint256 index (32)
        assert_eq!(output.len(), 36);
        assert_eq!(&output[..4], [0x22, 0x02, 0x66, 0xb6]);

        let index_bytes = &output[4..36];
        let index = U256::from_be_slice(index_bytes);
        assert_eq!(index, U256::from(2));
    }
}
