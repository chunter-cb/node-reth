//! Bridge implementation to adapt alloy providers to the EvmProvider trait

use alloy_primitives::{Address, Bytes, TxHash, B256, U256};
use aa_provider::{
    EvmProvider, ProviderResult, StateOverride, Block, BlockId, BlockNumberOrTag,
    FeeHistory, Filter, GasUsedResult, GethDebugTracingCallOptions, GethDebugTracingOptions,
    GethTrace, Log, TransactionReceipt, TransactionRequest, Transaction,
    DAGasOracle, DAGasOracleSync, ProviderError,
    RpcSend, RpcRecv, BlockHashOrNumber, TransactionBuilder,
};
use aa_types::{ExpectedStorage, da::{DAGasData, DAGasBlockData}};
use async_trait::async_trait;
use std::sync::Arc;

/// Adapter that implements EvmProvider for a standard alloy provider
#[derive(Clone)]
pub struct AlloyProviderAdapter<P> {
    provider: Arc<P>,
}

impl<P> AlloyProviderAdapter<P> {
    pub fn new(provider: Arc<P>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl<P> EvmProvider for AlloyProviderAdapter<P>
where
    P: alloy_provider::Provider<alloy_network::AnyNetwork> + Clone + Send + Sync + 'static,
{
    async fn request<Params, Resp>(&self, method: &'static str, params: Params) -> ProviderResult<Resp>
    where
        Params: RpcSend + 'static,
        Resp: RpcRecv,
    {
        self.provider
            .raw_request(method.into(), params)
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn fee_history(
        &self,
        block_count: u64,
        block_number: BlockNumberOrTag,
        reward_percentiles: &[f64],
    ) -> ProviderResult<FeeHistory> {
        self.provider
            .get_fee_history(block_count, block_number, reward_percentiles)
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn call(
        &self,
        tx: TransactionRequest,
        block: Option<BlockId>,
        state_overrides: Option<StateOverride>,
    ) -> ProviderResult<Bytes> {
        let mut call = self.provider.call(tx.into());
        
        if let Some(block) = block {
            call = call.block(block);
        }
        
        if let Some(overrides) = state_overrides {
            call = call.overrides(overrides);
        }
        
        call.await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn send_raw_transaction(&self, tx: Bytes) -> ProviderResult<TxHash> {
        self.provider
            .send_raw_transaction(&tx)
            .await
            .map(|pending| *pending.tx_hash())
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn send_raw_transaction_conditional(
        &self,
        tx: Bytes,
        _expected_storage: &ExpectedStorage,
    ) -> ProviderResult<TxHash> {
        // For now, just send normally. Conditional execution would need custom RPC endpoint
        self.send_raw_transaction(tx).await
    }

    async fn get_block_number(&self) -> ProviderResult<u64> {
        self.provider
            .get_block_number()
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn get_block(&self, block_id: BlockId) -> ProviderResult<Option<Block>> {
        self.provider
            .get_block(block_id)
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn get_full_block(&self, block_id: BlockId) -> ProviderResult<Option<Block>> {
        // Get block with full transactions
        let block = self.provider
            .get_block(block_id)
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))?;
        
        // For now return the same as get_block - would need separate handling for full transactions
        Ok(block)
    }

    async fn get_balance(&self, address: Address, block: Option<BlockId>) -> ProviderResult<U256> {
        let request = self.provider.get_balance(address);
        
        let result = if let Some(block_id) = block {
            request.block_id(block_id)
        } else {
            request
        };
        
        result.await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn get_transaction_by_hash(&self, tx: TxHash) -> ProviderResult<Option<Transaction>> {
        self.provider
            .get_transaction_by_hash(tx)
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn get_transaction_receipt(
        &self,
        tx: TxHash,
    ) -> ProviderResult<Option<TransactionReceipt>> {
        self.provider
            .get_transaction_receipt(tx)
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn debug_trace_transaction(
        &self,
        tx_hash: TxHash,
        trace_options: GethDebugTracingOptions,
    ) -> ProviderResult<GethTrace> {
        self.provider
            .raw_request("debug_traceTransaction".into(), (tx_hash, trace_options))
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn debug_trace_call(
        &self,
        tx: TransactionRequest,
        block_id: Option<BlockId>,
        trace_options: GethDebugTracingCallOptions,
    ) -> ProviderResult<GethTrace> {
        self.provider
            .raw_request("debug_traceCall".into(), (tx, block_id, trace_options))
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn get_latest_block_hash_and_number(&self) -> ProviderResult<(B256, u64)> {
        let block = self.provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))?
            .ok_or_else(|| ProviderError::from(anyhow::anyhow!("latest block not found")))?;
        
        Ok((block.header.hash, block.header.number))
    }

    async fn get_pending_block_hash_and_number(&self) -> ProviderResult<(B256, u64)> {
        let block = self.provider
            .get_block_by_number(BlockNumberOrTag::Pending)
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))?
            .ok_or_else(|| ProviderError::from(anyhow::anyhow!("pending block not found")))?;
        
        Ok((block.header.hash, block.header.number))
    }

    async fn get_pending_base_fee(&self) -> ProviderResult<u128> {
        let block = self.provider
            .get_block_by_number(BlockNumberOrTag::Pending)
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))?
            .ok_or_else(|| ProviderError::from(anyhow::anyhow!("pending block not found")))?;
        
        Ok(block.header.base_fee_per_gas.unwrap_or(0).into())
    }

    async fn get_max_priority_fee(&self) -> ProviderResult<u128> {
        self.provider
            .get_max_priority_fee_per_gas()
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn get_code(&self, address: Address, block: Option<BlockId>) -> ProviderResult<Bytes> {
        let request = self.provider.get_code_at(address);
        
        let result = if let Some(block_id) = block {
            request.block_id(block_id)
        } else {
            request
        };
        
        result.await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn get_transaction_count(&self, address: Address) -> ProviderResult<u64> {
        self.provider
            .get_transaction_count(address)
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn get_logs(&self, filter: &Filter) -> ProviderResult<Vec<Log>> {
        self.provider
            .get_logs(filter)
            .await
            .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))
    }

    async fn get_gas_used(&self, call: aa_provider::EvmCall) -> ProviderResult<GasUsedResult> {
        // This requires special handling with the GetGasUsed contract
        // For now, simulate with eth_call and return approximate values
        let result = self.call(
            TransactionRequest::default()
                .to(call.to)
                .value(call.value)
                .with_input(call.data.clone()),
            Some(BlockId::Number(BlockNumberOrTag::Latest)),
            Some(call.state_override),
        ).await;
        
        match result {
            Ok(data) => Ok(GasUsedResult {
                gasUsed: U256::from(100_000u64), // Approximate
                success: true,
                result: data,
            }),
            Err(_) => Ok(GasUsedResult {
                gasUsed: U256::from(100_000u64),
                success: false,
                result: Bytes::default(),
            }),
        }
    }

    async fn batch_get_storage_at(
        &self,
        address: Address,
        slots: Vec<B256>,
    ) -> ProviderResult<Vec<B256>> {
        let mut results = Vec::new();
        for slot in slots {
            let value = self.provider
                .get_storage_at(address, U256::from_be_bytes(slot.0))
                .await
                .map_err(|e| ProviderError::from(anyhow::anyhow!(e)))?;
            results.push(B256::from(value));
        }
        Ok(results)
    }

    async fn get_code_hash(
        &self,
        addresses: Vec<Address>,
        block: Option<BlockId>,
    ) -> ProviderResult<B256> {
        use alloy_primitives::keccak256;
        
        let mut code_hashes = Vec::new();
        for addr in addresses {
            let code = self.get_code(addr, block).await?;
            let hash = if code.is_empty() {
                B256::ZERO
            } else {
                B256::from(keccak256(&code))
            };
            code_hashes.push(hash);
        }
        
        // Hash all code hashes together
        let mut data = Vec::new();
        for hash in code_hashes {
            data.extend_from_slice(hash.as_slice());
        }
        Ok(keccak256(&data))
    }

    async fn get_balances(&self, addresses: Vec<Address>) -> ProviderResult<Vec<(Address, U256)>> {
        let mut results = Vec::new();
        for addr in addresses {
            let balance = self.get_balance(addr, None).await?;
            results.push((addr, balance));
        }
        Ok(results)
    }
}

/// Base DA gas oracle implementation
#[derive(Clone)]
pub struct BaseDAGasOracle;

impl BaseDAGasOracle {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl DAGasOracle for BaseDAGasOracle {
    async fn estimate_da_gas(
        &self,
        _data: Bytes,
        _to: Address,
        _block: BlockHashOrNumber,
        _gas_price: u128,
        _extra_data_len: usize,
    ) -> ProviderResult<(u128, DAGasData, DAGasBlockData)> {
        // Base mainnet DA gas calculation
        // For now return minimal values - you would implement actual Base DA gas calculation here
        Ok((
            0, // No additional DA gas for now
            DAGasData::default(),
            DAGasBlockData::default(),
        ))
    }
}

#[async_trait]
impl DAGasOracleSync for BaseDAGasOracle {
    async fn da_block_data(&self, _block: BlockHashOrNumber) -> ProviderResult<DAGasBlockData> {
        Ok(DAGasBlockData::default())
    }

    async fn da_gas_data(
        &self,
        _gas_data: Bytes,
        _to: Address,
        _block: BlockHashOrNumber,
    ) -> ProviderResult<DAGasData> {
        Ok(DAGasData::default())
    }

    fn calc_da_gas_sync(
        &self,
        _gas_data: &DAGasData,
        _block_data: &DAGasBlockData,
        _gas_price: u128,
        _extra_data_len: usize,
    ) -> u128 {
        0 // No additional DA gas for now
    }
}

// Note: The AlloyProviderAdapter cannot directly implement alloy_provider::Provider
// because that would create conflicting trait implementations. 
// 
// To properly integrate gas estimation, you'll need to either:
// 1. Create a custom provider that implements both traits
// 2. Use the rundler provider implementation directly
// 3. Create wrapper types that handle the conversion
// 
// The key challenge is that aa_provider expects its own Provider trait (EvmProvider)
// while the EntryPoint implementations expect alloy_provider::Provider.