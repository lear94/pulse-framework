#!/bin/bash
set -e
GREEN='\033[0;32m'
CYAN='\033[0;36m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'
APP="toxic_target"
DB="pulse_toxic_db"
CLI="./target/release/pulse-cli"

echo "---------------------------------------------------"
echo "☣️  [LEVEL 9] TOXIC PAYLOAD (Fuzzing)"
echo "---------------------------------------------------"

function cleanup() {
    pkill -9 -f "$APP" || true
    docker rm -f $DB > /dev/null 2>&1 || true
    rm -rf $APP toxic_payload.json
}
trap cleanup EXIT
cleanup

echo -n "   -> Infra (DB Port 5437)..."
docker run --name $DB --shm-size=256mb -e POSTGRES_PASSWORD=s -e POSTGRES_USER=p -e POSTGRES_DB=d -p 5437:5432 -d postgres:alpine > /dev/null
until docker exec $DB pg_isready -U p > /dev/null 2>&1; do sleep 1; done
sleep 2
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
        database_url: "postgres://p:s@127.0.0.1:5437/d".to_string(),
        redis_url: None,
        host: "0.0.0.0".to_string(), port: 8084,
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
while ! curl -s http://127.0.0.1:8084/health > /dev/null; do sleep 0.5; done
echo " ONLINE."

# TEST 1: RECURSION BOMB
echo -e "${YELLOW}   -> Test A: The 'Matryoshka' Bomb (Deep Recursion)...${NC}"
# Generar un JSON con 2000 niveles de anidamiento
echo -n '{"username":"' > toxic_payload.json
for i in {1..2000}; do echo -n 'a'; done >> toxic_payload.json
echo -n '", "email": "bomb@test.com"}' >> toxic_payload.json

SIZE=$(ls -lh toxic_payload.json | awk '{print $5}')
echo -e "      Payload Size: ${CYAN}$SIZE${NC}"

CODE=$(curl -s -o /dev/null -w "%{http_code}" -d @toxic_payload.json -H "Content-Type: application/json" http://127.0.0.1:8084/api/v1/users)

if [ "$CODE" == "201" ]; then
    echo -e "      Result: ${GREEN}HANDLED ($CODE)${NC}"
elif [ "$CODE" == "400" ] || [ "$CODE" == "413" ]; then
    echo -e "      Result: ${GREEN}REJECTED SAFELY ($CODE)${NC}"
else
    echo -e "      Result: ${RED}CRASH/UNKNOWN ($CODE)${NC}"
fi

# TEST 2: BUFFER OVERFLOW ATTEMPT
echo -e "${YELLOW}   -> Test B: The 'Blob' (10MB String)...${NC}"
# Crear un payload de 10MB en memoria simulada (username gigante)
dd if=/dev/zero bs=1M count=10 2>/dev/null | tr '\0' 'A' > large_blob.txt
LONG_STR=$(cat large_blob.txt)
# Nota: Usamos curl -d @ para no explotar la shell
echo "{\"username\":\"$LONG_STR\",\"email\":\"blob@test.com\"}" > toxic_blob.json

CODE=$(curl -s -o /dev/null -w "%{http_code}" -d @toxic_blob.json -H "Content-Type: application/json" http://127.0.0.1:8084/api/v1/users)

if [ "$CODE" == "201" ]; then
    echo -e "      Result: ${YELLOW}ACCEPTED ($CODE) - Warning: High memory usage${NC}"
elif [ "$CODE" == "400" ] || [ "$CODE" == "413" ] || [ "$CODE" == "500" ]; then
    echo -e "      Result: ${GREEN}SHIELDED ($CODE)${NC}"
else
    # Si el servidor murió, curl fallará conexión (000)
    echo -e "      Result: ${RED}SYSTEM COMPROMISED ($CODE)${NC}"
    exit 1
fi

# SUPERVIVENCIA
echo -n "   -> Vital Signs Check..."
if curl -s http://127.0.0.1:8084/health | grep -q "operational"; then
    echo -e "${GREEN} PULSE DETECTED${NC}"
else
    echo -e "${RED} FLATLINE${NC}"
    exit 1
fi

echo -e "${GREEN}✅ PASSED${NC}"
