# Account Abstraction Bundler E2E Test

This guide walks through testing the native AA bundler end-to-end using the local playground environment.

## Prerequisites

- Docker installed and running
- [builder-playground-aa](https://github.com/flashbots/builder-playground) cloned and built
- Foundry installed (`cast` command available)

## Steps

### 1. Build Docker Images

From the `node-reth` repository root:

```bash
# Build both images (takes ~5-10 minutes first time)
docker build --platform linux/arm64 -t op-rbuilder-aa:latest -f Dockerfile.op-rbuilder .
docker build --platform linux/arm64 -t base-reth-node:latest -f Dockerfile .
```

### 2. Start Playground

```bash
cd /path/to/builder-playground-aa
./playground-bin start opstack --aa --flashblocks --external-builder op-rbuilder
```

Wait for "All services are healthy!" message.

### 3. Deploy EntryPoint Contracts

```bash
cd node-reth/crates/client/account-abstraction
./deploy_contracts.sh
```

This deploys v0.6, v0.7, and v0.8 EntryPoint contracts.

### 4. Send a UserOperation

```bash
./send_userop.sh
```

On success, you'll see:
```
✓ UserOperation submitted!
UserOp Hash: 0x...
```

### 5. Verify Receipt

Wait ~5 seconds for the bundler to include the UserOp, then check the receipt:

```bash
curl -X POST http://localhost:8547 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_getUserOperationReceipt","params":["<USEROP_HASH>"],"id":1}' | jq .
```

A successful response shows:
```json
{
  "result": {
    "userOpHash": "0x...",
    "success": true,
    "actualGasUsed": "0x...",
    ...
  }
}
```

## Flow Summary

```
User → base-reth-node (eth_sendUserOperation)
         ↓
      Mempool + P2P Gossip
         ↓
      op-rbuilder receives UserOp
         ↓
      Bundler creates handleOps tx
         ↓
      Block includes bundle
         ↓
      Receipt available via RPC
```

## Troubleshooting

**UserOp not being bundled?**
- Check p2p connection: `curl -X POST http://localhost:8549 -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","method":"debug_bundler_getPeers","params":[],"id":1}'`
- If no peers, manually connect: `curl -X POST http://localhost:8549 -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","method":"debug_bundler_connectPeer","params":["/ip4/<BASE_RETH_IP>/tcp/9545/p2p/<PEER_ID>"],"id":1}'`

**"Replacement gas too low" error?**
- A UserOp with the same sender/nonce already exists. Use a different owner key:
  ```bash
  OWNER_KEY="0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a" ./send_userop.sh
  ```

## Cleanup

```bash
cd /path/to/builder-playground-aa
./playground-bin clean all
```
