#!/bin/bash
set -e
GREEN='\033[0;32m'
CYAN='\033[0;36m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'
APP="idempotency_target"
DB="pulse_idem_db"
CLI="./target/release/pulse-cli"

echo "---------------------------------------------------"
echo "🧟 [LEVEL 8] IDEMPOTENCY (The Double Zombie)"
echo "---------------------------------------------------"

function cleanup() {
    pkill -9 -f "$APP" || true
    docker rm -f $DB > /dev/null 2>&1 || true
    rm -rf $APP
}
trap cleanup EXIT
cleanup

echo -n "   -> Infra (DB Port 5436)..."
docker run --name $DB --shm-size=256mb -e POSTGRES_PASSWORD=s -e POSTGRES_USER=p -e POSTGRES_DB=d -p 5436:5432 -d postgres:alpine > /dev/null
until docker exec $DB pg_isready -U p > /dev/null 2>&1; do sleep 1; done
sleep 2
# Schema Injection
docker exec -i $DB psql -U p -d d -c "CREATE TABLE users (id UUID PRIMARY KEY, username VARCHAR, email VARCHAR, created_at TIMESTAMP);" > /dev/null
docker exec -i $DB psql -U p -d d -c "CREATE TABLE blackbox_records (id UUID PRIMARY KEY, handler VARCHAR, payload JSONB, error VARCHAR, timestamp TIMESTAMP WITH TIME ZONE);" > /dev/null
echo " OK."

# App Setup
$CLI new $APP > /dev/null
echo >> $APP/Cargo.toml; echo '[workspace]' >> $APP/Cargo.toml
cat << 'RUST' > $APP/src/main.rs
use pulse_core::{bootstrap, PulseConfig, utoipa::OpenApi};
use pulse_core::api::{config as api_routes, ApiDoc};
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let config = PulseConfig {
        database_url: "postgres://p:s@127.0.0.1:5436/d".to_string(),
        redis_url: None,
        host: "0.0.0.0".to_string(), port: 8083,
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
while ! curl -s http://127.0.0.1:8083/health > /dev/null; do sleep 0.5; done
echo " ONLINE."

# 1. Crear usuario legítimo
echo -e "${CYAN}   -> Step 1: Creating legitimate user 'PatientZero'...${NC}"
curl -s -X POST http://127.0.0.1:8083/api/v1/users \
     -H "Content-Type: application/json" \
     -d '{"username":"PatientZero","email":"zero@labs.com"}' > /dev/null

# 2. Inyectar manualmente un fallo falso en Blackbox (El usuario ya existe, pero fingimos que falló)
echo -e "${CYAN}   -> Step 2: Injecting Fake Failure record...${NC}"
FAKE_ID="550e8400-e29b-41d4-a716-446655440000"
PAYLOAD='{"username":"PatientZero","email":"zero@labs.com"}' # Mismo email, debería chocar
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
SQL="INSERT INTO blackbox_records (id, handler, payload, error, timestamp) VALUES ('$FAKE_ID', 'create_user', '$PAYLOAD', 'Simulated Crash', '$TIMESTAMP');"
docker exec -i $DB psql -U p -d d -c "$SQL" > /dev/null

# 3. Intentar Replay (Debe detectar duplicado)
echo -e "${YELLOW}   -> Step 3: Triggering Lazarus Protocol on existing data...${NC}"
RESPONSE=$(curl -s -X POST http://127.0.0.1:8083/api/v1/admin/replay/$FAKE_ID)

echo -e "      Response: $(echo $RESPONSE | cut -c 1-60)..."

# Verificaciones
if echo "$RESPONSE" | grep -q "error"; then
    echo -e "${GREEN}   -> SUCCESS: System correctly rejected the duplicate.${NC}"
    
    # Verificar que solo hay 1 usuario en la DB real (Integridad de datos)
    COUNT=$(docker exec $DB psql -U p -d d -t -c "SELECT count(*) FROM users WHERE email='zero@labs.com';" | tr -d ' ')
    if [ "$COUNT" -eq "1" ]; then
        echo -e "      Data Integrity: ${GREEN}1 Record (Correct)${NC}"
        echo -e "${GREEN}✅ PASSED${NC}"
    else
        echo -e "      Data Integrity: ${RED}$COUNT Records (CORRUPTED)${NC}"
        exit 1
    fi
else
    echo -e "${RED}   -> FAIL: System allowed duplication or crashed.${NC}"
    exit 1
fi
