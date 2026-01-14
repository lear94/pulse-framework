#!/bin/bash
set -e
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'
APP="security_target"
DB="pulse_sec_db"
CLI="./target/release/pulse-cli"

echo "---------------------------------------------------"
echo "🛡️  [LEVEL 4] SECURITY AUDIT"
echo "---------------------------------------------------"

function cleanup() {
    pkill -9 -f "$APP" || true
    docker rm -f $DB > /dev/null 2>&1 || true
    rm -rf $APP
}
trap cleanup EXIT
cleanup

echo -n "   -> Starting DB..."
# [FIX] Added shm-size
docker run --name $DB --shm-size=256mb -e POSTGRES_PASSWORD=s -e POSTGRES_USER=p -e POSTGRES_DB=d -p 5434:5432 -d postgres:alpine > /dev/null
until docker exec $DB pg_isready -U p > /dev/null 2>&1; do sleep 1; done
# [FIX] Stabilization sleep
sleep 2
echo " OK."

$CLI new $APP > /dev/null
echo >> $APP/Cargo.toml; echo '[workspace]' >> $APP/Cargo.toml
cat << 'RUST' > $APP/src/main.rs
use pulse_core::{bootstrap, PulseConfig, utoipa::OpenApi};
use pulse_core::api::{config as api_routes, ApiDoc};
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let config = PulseConfig {
        database_url: "postgres://p:s@127.0.0.1:5434/d".to_string(), 
        redis_url: None,
        host: "0.0.0.0".to_string(), port: 8081,
        db_max_connections: 10
    };
    bootstrap(config, api_routes, ApiDoc::openapi()).await
}
RUST

cd $APP
cargo build --release --quiet
RUST_LOG=off ./target/release/$APP > /dev/null 2>&1 &
SERVER_PID=$!
cd ..

echo -n "   -> Waiting App..."
COUNT=0
while ! curl -s http://127.0.0.1:8081/health > /dev/null; do
    sleep 0.5
    COUNT=$((COUNT+1))
    if [ $COUNT -ge 20 ]; then echo -e "${RED} TIMEOUT${NC}"; exit 1; fi
done
echo " OK."

echo -n "   -> Testing Unauthorized Access..."
CODE=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8081/api/v1/users)
if [ "$CODE" == "401" ]; then echo -e "${GREEN} OK${NC}"; else echo -e "${RED} FAIL ($CODE)${NC}"; exit 1; fi

echo -e "${GREEN}✅ PASSED${NC}"
