#!/bin/bash
set -e
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'
APP="recovery_target"
DB="pulse_rec_db"
CLI="./target/release/pulse-cli"

echo "---------------------------------------------------"
echo "💾 [LEVEL 5] BLACKBOX RECOVERY"
echo "---------------------------------------------------"

function cleanup() {
    pkill -9 -f "$APP" || true
    docker rm -f $DB > /dev/null 2>&1 || true
    rm -rf $APP storage/blackbox.jsonl
}
trap cleanup EXIT
cleanup

echo -n "   -> Starting DB..."
# [FIX] Added shm-size
docker run --name $DB --shm-size=256mb -e POSTGRES_PASSWORD=s -e POSTGRES_USER=p -e POSTGRES_DB=d -p 5435:5432 -d postgres:alpine > /dev/null
until docker exec $DB pg_isready -U p > /dev/null 2>&1; do sleep 1; done
# [FIX] Stabilization sleep before schema injection
sleep 2
docker exec -i $DB psql -U p -d d -c "CREATE TABLE users (id UUID PRIMARY KEY, username VARCHAR, email VARCHAR, created_at TIMESTAMP);" > /dev/null
docker exec -i $DB psql -U p -d d -c "CREATE TABLE blackbox_records (id UUID PRIMARY KEY, handler VARCHAR, payload JSONB, error VARCHAR, timestamp TIMESTAMP WITH TIME ZONE);" > /dev/null
echo " OK."

$CLI new $APP > /dev/null
echo >> $APP/Cargo.toml; echo '[workspace]' >> $APP/Cargo.toml
cat << 'RUST' > $APP/src/main.rs
use pulse_core::{bootstrap, PulseConfig, utoipa::OpenApi};
use pulse_core::api::{config as api_routes, ApiDoc};
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let config = PulseConfig {
        database_url: "postgres://p:s@127.0.0.1:5435/d".to_string(),
        redis_url: None,
        host: "0.0.0.0".to_string(), port: 8082,
        db_max_connections: 10
    };
    bootstrap(config, api_routes, ApiDoc::openapi()).await
}
RUST

cd $APP
cargo build --release --quiet
RUST_LOG=error ./target/release/$APP > /dev/null 2>&1 &
SERVER_PID=$!
cd ..

echo -n "   -> Waiting App..."
sleep 3
while ! curl -s http://127.0.0.1:8082/health > /dev/null; do sleep 1; done
echo " ONLINE."

echo -n "   -> ⚡ SABOTAGE: Killing DB Container..."
docker stop $DB
echo " DONE."

echo -n "   -> Sending Payload..."
curl -s -X POST http://127.0.0.1:8082/api/v1/users \
     -H "Content-Type: application/json" \
     -d '{"username":"survivor","email":"hope@future.com"}' > /dev/null
sleep 1

# Check correct path inside app
if grep -q "survivor" $APP/storage/blackbox.jsonl 2>/dev/null; then
    echo -e "${GREEN} OK (Saved)${NC}"
else
    echo -e "${RED} FAIL (Lost)${NC}"; exit 1
fi

echo -e "${GREEN}✅ PASSED${NC}"
