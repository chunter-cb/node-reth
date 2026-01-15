#!/usr/bin/env bash
# Send a test UserOperation to deploy a SimpleAccount
#
# Usage:
#   ./send_userop.sh
#
# Environment variables:
#   RPC_URL           - RPC endpoint for UserOp submission (default: http://localhost:8547)
#   TX_RPC_URL        - RPC endpoint for transactions (default: http://localhost:8549)
#   OWNER_KEY         - Private key of the account owner (default: test account #2)
#   FUNDER_KEY        - Private key to fund the account (default: test account #1)
#   ENTRYPOINT        - EntryPoint address (default: v0.6)
#   FACTORY           - SimpleAccountFactory address

set -e

# Configuration
RPC_URL="${RPC_URL:-http://localhost:8547}"
TX_RPC_URL="${TX_RPC_URL:-http://localhost:8549}"

# Test mnemonic accounts
# Account #1 (funder): 0x70997970C51812dc3A010C7d01b50e0d17dc79C8
FUNDER_KEY="${FUNDER_KEY:-0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d}"

# Account #2 (owner): 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC
OWNER_KEY="${OWNER_KEY:-0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a}"
# Derive OWNER address from OWNER_KEY if not explicitly set
if [ -z "${OWNER:-}" ]; then
    OWNER=$(cast wallet address --private-key "$OWNER_KEY" 2>/dev/null || echo "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC")
fi

# Default addresses
ENTRYPOINT="${ENTRYPOINT:-0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789}"
FACTORY="${FACTORY:-0x9406Cc6185a346906296840746125a0E44976454}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Find cast
CAST_CMD="cast"
if ! command -v cast &> /dev/null; then
    if [ -f "$HOME/.foundry/bin/cast" ]; then
        CAST_CMD="$HOME/.foundry/bin/cast"
    else
        echo -e "${RED}Error: 'cast' (foundry) is required${NC}"
        echo "Install: curl -L https://foundry.paradigm.xyz | bash && foundryup"
        exit 1
    fi
fi

echo -e "${BLUE}════════════════════════════════════════════════════════════${NC}"
echo -e "${BLUE}  Send Test UserOperation${NC}"
echo -e "${BLUE}════════════════════════════════════════════════════════════${NC}"
echo ""
echo -e "UserOp RPC:  ${YELLOW}$RPC_URL${NC}"
echo -e "TX RPC:      ${YELLOW}$TX_RPC_URL${NC}"
echo -e "EntryPoint:  ${YELLOW}$ENTRYPOINT${NC}"
echo -e "Factory:     ${YELLOW}$FACTORY${NC}"
echo -e "Owner:       ${YELLOW}$OWNER${NC}"
echo ""

# Step 1: Get counterfactual account address
echo -e "${BLUE}[1/6] Getting counterfactual account address...${NC}"
SENDER=$($CAST_CMD call $FACTORY \
    "getAddress(address,uint256)(address)" \
    $OWNER 0 \
    --rpc-url $TX_RPC_URL)
echo -e "  Account: ${GREEN}$SENDER${NC}"

# Step 2: Check if account is already deployed
echo -e "${BLUE}[2/6] Checking account status...${NC}"
CODE=$(curl -s -X POST "$TX_RPC_URL" \
    -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getCode\",\"params\":[\"$SENDER\", \"latest\"],\"id\":1}" \
    | grep -o '"result":"[^"]*"' | cut -d'"' -f4)

if [ "$CODE" != "0x" ] && [ -n "$CODE" ] && [ ${#CODE} -gt 4 ]; then
    echo -e "  ${YELLOW}Account already deployed${NC}"
    INIT_CODE="0x"
else
    echo -e "  Account not deployed, will include initCode"
    INIT_CODE="0x${FACTORY:2}5fbfb9cf000000000000000000000000${OWNER:2}0000000000000000000000000000000000000000000000000000000000000000"
fi

# Step 3: Check/create deposit
echo -e "${BLUE}[3/6] Checking EntryPoint deposit...${NC}"
DEPOSIT=$($CAST_CMD call $ENTRYPOINT \
    "balanceOf(address)(uint256)" $SENDER \
    --rpc-url $TX_RPC_URL)
echo -e "  Current deposit: $DEPOSIT wei"

if [ "$DEPOSIT" = "0" ]; then
    echo -e "  Depositing 0.1 ETH to EntryPoint..."
    $CAST_CMD send \
        --private-key $FUNDER_KEY \
        --rpc-url $TX_RPC_URL \
        $ENTRYPOINT \
        "depositTo(address)" $SENDER \
        --value 0.1ether > /dev/null
    sleep 3
    echo -e "  ${GREEN}Deposited${NC}"
fi

# Step 4: Get nonce
echo -e "${BLUE}[4/6] Getting nonce...${NC}"
NONCE=$($CAST_CMD call $ENTRYPOINT \
    "getNonce(address,uint192)(uint256)" \
    $SENDER 0 \
    --rpc-url $TX_RPC_URL)
echo -e "  Nonce: $NONCE"

# Step 5: Compute UserOp hash and sign
echo -e "${BLUE}[5/6] Computing UserOp hash and signing...${NC}"

# Gas values
CALL_GAS_LIMIT="0x10000"
VERIFICATION_GAS_LIMIT="0x80000"
PRE_VERIFICATION_GAS="0x10000"
MAX_FEE="0x77359400"
MAX_PRIORITY_FEE="0x77359400"

# Empty callData for account deployment only
CALL_DATA="0x"

# Compute UserOp hash
USER_OP_HASH=$($CAST_CMD call $ENTRYPOINT \
    "getUserOpHash((address,uint256,bytes,bytes,uint256,uint256,uint256,uint256,uint256,bytes,bytes))" \
    "($SENDER,$NONCE,$INIT_CODE,$CALL_DATA,$CALL_GAS_LIMIT,$VERIFICATION_GAS_LIMIT,$PRE_VERIFICATION_GAS,$MAX_FEE,$MAX_PRIORITY_FEE,0x,0x)" \
    --rpc-url $TX_RPC_URL)
echo -e "  UserOp hash: $USER_OP_HASH"

# Sign the hash
SIGNATURE=$($CAST_CMD wallet sign --private-key $OWNER_KEY "$USER_OP_HASH")
echo -e "  Signature: ${SIGNATURE:0:20}..."

# Step 6: Send UserOperation
echo -e "${BLUE}[6/6] Sending UserOperation...${NC}"
RESULT=$(curl -s -X POST "$RPC_URL" \
    -H "Content-Type: application/json" \
    -d "{
        \"jsonrpc\": \"2.0\",
        \"method\": \"eth_sendUserOperation\",
        \"params\": [{
            \"sender\": \"$SENDER\",
            \"nonce\": \"0x$(printf '%x' $NONCE)\",
            \"initCode\": \"$INIT_CODE\",
            \"callData\": \"$CALL_DATA\",
            \"callGasLimit\": \"$CALL_GAS_LIMIT\",
            \"verificationGasLimit\": \"$VERIFICATION_GAS_LIMIT\",
            \"preVerificationGas\": \"$PRE_VERIFICATION_GAS\",
            \"maxFeePerGas\": \"$MAX_FEE\",
            \"maxPriorityFeePerGas\": \"$MAX_PRIORITY_FEE\",
            \"paymasterAndData\": \"0x\",
            \"signature\": \"$SIGNATURE\"
        }, \"$ENTRYPOINT\"],
        \"id\": 1
    }")

# Check result
if echo "$RESULT" | grep -q '"result"'; then
    USEROP_HASH=$(echo "$RESULT" | grep -o '"result":"[^"]*"' | cut -d'"' -f4)
    echo -e "  ${GREEN}✓ UserOperation submitted!${NC}"
    echo -e "  UserOp Hash: ${GREEN}$USEROP_HASH${NC}"
    echo ""
    echo -e "${BLUE}════════════════════════════════════════════════════════════${NC}"
    echo -e "${GREEN}  Success!${NC}"
    echo -e "${BLUE}════════════════════════════════════════════════════════════${NC}"
    echo ""
    echo -e "UserOp is now in the mempool waiting to be bundled."
    echo -e "Check status with:"
    echo -e "  curl -X POST $RPC_URL -H 'Content-Type: application/json' \\"
    echo -e "    -d '{\"jsonrpc\":\"2.0\",\"method\":\"eth_getUserOperationByHash\",\"params\":[\"$USEROP_HASH\"],\"id\":1}'"
else
    ERROR=$(echo "$RESULT" | grep -o '"message":"[^"]*"' | cut -d'"' -f4)
    echo -e "  ${RED}✗ Failed: $ERROR${NC}"
    echo "$RESULT" | jq . 2>/dev/null || echo "$RESULT"
    exit 1
fi
