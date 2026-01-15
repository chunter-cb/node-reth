#!/usr/bin/env bash
# Deploy ERC-4337 EntryPoints and SimpleAccountFactories
# Uses the eth-infinitism hardhat-deploy scripts for deterministic CREATE2 deployment
#
# Usage:
#   ./deploy_contracts.sh [versions]
#
# Examples:
#   ./deploy_contracts.sh                  # Deploy all versions (0.6, 0.7, 0.8)
#   ./deploy_contracts.sh 0.6              # Deploy only v0.6
#   ./deploy_contracts.sh 0.6 0.7          # Deploy v0.6 and v0.7
#   VERSIONS="0.6,0.7" ./deploy_contracts.sh  # Using env var
#   ACCOUNT_INDEX=5 ./deploy_contracts.sh  # Use account #5 instead of #0
#
# Environment variables:
#   RPC_URL       - RPC endpoint (default: http://localhost:8549)
#   VERSIONS      - Comma-separated versions to deploy (default: 0.6,0.7,0.8)
#   ACCOUNT_INDEX - Which account from mnemonic to use (default: 5)
#                   Accounts 0-4 may be used by other services (builder, batcher, etc.)

set -e

# Configuration
# Default to op-rbuilder port for transaction inclusion
RPC_URL="${RPC_URL:-http://localhost:8549}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_DIR="$SCRIPT_DIR/contracts/solidity"

# Parse versions from args or env var
if [ $# -gt 0 ]; then
    VERSIONS="$*"
    VERSIONS="${VERSIONS// /,}"  # Replace spaces with commas
else
    VERSIONS="${VERSIONS:-0.6,0.7,0.8}"
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Check requirements
if ! command -v node &> /dev/null; then
    echo -e "${RED}Error: 'node' is required but not installed${NC}"
    exit 1
fi

if ! command -v yarn &> /dev/null; then
    echo -e "${RED}Error: 'yarn' is required but not installed${NC}"
    echo "Install: npm install -g yarn"
    exit 1
fi

echo -e "${BLUE}════════════════════════════════════════════════════════════${NC}"
echo -e "${BLUE}  ERC-4337 Contract Deployment Script${NC}"
echo -e "${BLUE}  (Using deterministic CREATE2 deployment)${NC}"
echo -e "${BLUE}════════════════════════════════════════════════════════════${NC}"
echo ""
echo -e "RPC URL:  ${YELLOW}$RPC_URL${NC}"
echo -e "Versions: ${YELLOW}$VERSIONS${NC}"
echo ""

# Deploy deterministic deployer if not present
deploy_deterministic_deployer() {
    local DEPLOYER_ADDR="0x4e59b44847b379578588920cA78FbF26c0B4956C"
    local FUNDER_ADDR="0x3fab184622dc19b6109349b94811493bf2a45362"
    
    echo -e "${BLUE}Checking deterministic deployer...${NC}"
    
    # Check if already deployed
    local code=$(curl -s -X POST "$RPC_URL" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getCode\",\"params\":[\"$DEPLOYER_ADDR\", \"latest\"],\"id\":1}" \
        | grep -o '"result":"[^"]*"' | cut -d'"' -f4)
    
    if [ "$code" != "0x" ] && [ -n "$code" ] && [ ${#code} -gt 4 ]; then
        echo -e "  ${GREEN}✓ Deterministic deployer already deployed${NC}"
        return 0
    fi
    
    echo -e "  Deploying deterministic deployer (CREATE2 factory)..."
    
    # Check if cast is available
    local CAST_CMD="cast"
    if ! command -v cast &> /dev/null; then
        if [ -f "$HOME/.foundry/bin/cast" ]; then
            CAST_CMD="$HOME/.foundry/bin/cast"
        else
            echo -e "  ${RED}Error: 'cast' (foundry) is required for deploying deterministic deployer${NC}"
            echo -e "  Install foundry: curl -L https://foundry.paradigm.xyz | bash && foundryup"
            return 1
        fi
    fi
    
    # Use test mnemonic account #5 to fund the deployer (accounts 0-4 may be used by other services)
    # Account 5: 0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc
    local PRIVATE_KEY="${DEPLOYER_PRIVATE_KEY:-0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba}"
    
    # Fund the deterministic deployer funder address
    echo -e "  Funding deployer address..."
    $CAST_CMD send --private-key "$PRIVATE_KEY" \
        --rpc-url "$RPC_URL" \
        "$FUNDER_ADDR" \
        --value 1ether \
        --timeout 30 > /dev/null 2>&1 || {
            echo -e "  ${YELLOW}Warning: Funding transaction may have failed, continuing anyway...${NC}"
        }
    
    sleep 2
    
    # Deploy deterministic deployer using the pre-signed raw transaction
    echo -e "  Broadcasting deployer contract..."
    $CAST_CMD publish --rpc-url "$RPC_URL" \
        "0xf8a58085174876e800830186a08080b853604580600e600039806000f350fe7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe03601600081602082378035828234f58015156039578182fd5b8082525050506014600cf31ba02222222222222222222222222222222222222222222222222222222222222222a02222222222222222222222222222222222222222222222222222222222222222" \
        > /dev/null 2>&1 || {
            echo -e "  ${YELLOW}Warning: Deploy transaction may have failed (might already exist)${NC}"
        }
    
    sleep 3
    
    # Verify deployment
    code=$(curl -s -X POST "$RPC_URL" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getCode\",\"params\":[\"$DEPLOYER_ADDR\", \"latest\"],\"id\":1}" \
        | grep -o '"result":"[^"]*"' | cut -d'"' -f4)
    
    if [ "$code" != "0x" ] && [ -n "$code" ] && [ ${#code} -gt 4 ]; then
        echo -e "  ${GREEN}✓ Deterministic deployer deployed at $DEPLOYER_ADDR${NC}"
        return 0
    else
        echo -e "  ${RED}✗ Failed to deploy deterministic deployer${NC}"
        return 1
    fi
}

# Deploy deterministic deployer first
deploy_deterministic_deployer || exit 1
echo ""

# Function to deploy using hardhat with inline network config
deploy_version() {
    local version=$1
    local submodule_dir=$2
    
    echo -e "${BLUE}Deploying v$version...${NC}"
    
    cd "$submodule_dir"
    
    # Clean up any stale pending transactions from previous failed deployments
    # This prevents hardhat-deploy from trying to re-broadcast old transactions
    if [ -f "deployments/localreth/.pendingTransactions" ]; then
        echo -e "  Cleaning up stale pending transactions..."
        rm -f "deployments/localreth/.pendingTransactions"
    fi
    
    # Install dependencies if needed
    if [ ! -d "node_modules" ]; then
        echo -e "  Installing dependencies (this may take a minute)..."
        yarn install --ignore-engines
    fi
    
    # Create a temporary hardhat config that imports the original and adds our network
    # Use ACCOUNT_INDEX to derive a different account from the mnemonic (default: 5)
    # Accounts 0-4 may be used by other services (builder, batcher, etc.)
    local account_index="${ACCOUNT_INDEX:-5}"
    cat > hardhat.config.local.ts << CONFIGEOF
import baseConfig from './hardhat.config'
import { HardhatUserConfig } from 'hardhat/config'

const config: HardhatUserConfig = {
  ...baseConfig,
  networks: {
    ...baseConfig.networks,
    localreth: {
      url: process.env.RPC_URL || 'http://localhost:8547',
      accounts: {
        mnemonic: 'test test test test test test test test test test test junk',
        path: "m/44'/60'/0'/0",
        initialIndex: ${account_index},
        count: 1
      }
    }
  }
}

export default config
CONFIGEOF

    # Temporarily patch the SimpleAccountFactory deploy script to allow any chain ID
    local factory_deploy="deploy/2_deploy_SimpleAccountFactory.ts"
    local factory_backup="/tmp/2_deploy_SimpleAccountFactory.ts.bak.$$"
    if [ -f "$factory_deploy" ]; then
        cp "$factory_deploy" "$factory_backup"
        # Remove the chain ID check (replace the if block that returns early)
        sed -i.tmp 's/if (network.chainId !== 31337 && network.chainId !== 1337)/if (false)/' "$factory_deploy"
        rm -f "${factory_deploy}.tmp"
    fi

    # Run deploy with our network using the local config
    echo -e "  Running hardhat deploy..."
    RPC_URL="$RPC_URL" npx hardhat deploy --network localreth --config hardhat.config.local.ts 2>&1 | while read line; do
        echo "  $line"
    done
    local deploy_status=${PIPESTATUS[0]}
    
    # Clean up temporary config
    rm -f hardhat.config.local.ts
    
    # Restore original factory deploy script
    if [ -f "$factory_backup" ]; then
        mv "$factory_backup" "$factory_deploy"
    fi
    
    if [ $deploy_status -ne 0 ]; then
        echo -e "  ${RED}✗ v$version deployment failed${NC}"
        return 1
    fi
    
    echo -e "  ${GREEN}✓ v$version deployed${NC}"
    echo ""
}

# Convert versions string to array and deploy each
IFS=',' read -ra VERSION_ARRAY <<< "$VERSIONS"
TOTAL=${#VERSION_ARRAY[@]}
COUNT=0

for version in "${VERSION_ARRAY[@]}"; do
    # Trim whitespace
    version=$(echo "$version" | tr -d ' ')
    COUNT=$((COUNT + 1))
    
    case "$version" in
        0.6|v0.6)
            echo -e "${BLUE}[$COUNT/$TOTAL] Deploying v0.6 contracts...${NC}"
            deploy_version "0.6" "$CONTRACTS_DIR/v0_6/lib/account-abstraction"
            ;;
        0.7|v0.7)
            echo -e "${BLUE}[$COUNT/$TOTAL] Deploying v0.7 contracts...${NC}"
            deploy_version "0.7" "$CONTRACTS_DIR/v0_7/lib/account-abstraction"
            ;;
        0.8|v0.8)
            echo -e "${BLUE}[$COUNT/$TOTAL] Deploying v0.8 contracts...${NC}"
            deploy_version "0.8" "$CONTRACTS_DIR/v0_8/lib/account-abstraction"
            ;;
        *)
            echo -e "${RED}Unknown version: $version (valid: 0.6, 0.7, 0.8)${NC}"
            exit 1
            ;;
    esac
done

# Summary
echo -e "${BLUE}════════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  Deployment Complete!${NC}"
echo -e "${BLUE}════════════════════════════════════════════════════════════${NC}"
echo ""
echo -e "${YELLOW}Expected canonical addresses:${NC}"
echo -e "  EntryPoint v0.6:     0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789"
echo -e "  EntryPoint v0.7:     0x0000000071727De22E5E9d8BAf0edAc6f37da032"
echo -e "  EntryPoint v0.8:     0x4337084D9e255Ff0702461CF8895CE9E3B5Ff108"
echo ""
echo -e "${YELLOW}Note:${NC} Check deployment output above for actual addresses."
echo -e "Deployments are saved to each submodule's deployments/ folder."
