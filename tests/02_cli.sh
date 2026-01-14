#!/bin/bash
set -e
GREEN='\033[0;32m'
NC='\033[0m'
TEST_PROJ="pulse_cli_test"

echo "---------------------------------------------------"
echo "🛠️  [LEVEL 2] CLI TOOLING CHECK"
echo "---------------------------------------------------"

echo -n "   -> Building CLI..."
cargo build --release -p pulse-cli --quiet
CLI="./target/release/pulse-cli"
echo -e "${GREEN} OK${NC}"

echo -n "   -> Scaffolding..."
rm -rf $TEST_PROJ
$CLI new $TEST_PROJ > /dev/null
echo >> $TEST_PROJ/Cargo.toml; echo '[workspace]' >> $TEST_PROJ/Cargo.toml

echo -n "   -> Verifying..."
if cargo check --quiet; then echo -e "${GREEN} OK${NC}"; else echo "FAIL"; exit 1; fi

rm -rf $TEST_PROJ
echo -e "${GREEN}✅ PASSED${NC}"
