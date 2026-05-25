#!/bin/bash
set -e
export JWT_SECRET="pulse-certification-secret-key"
# Sube el rate limit para que la prueba de carga golpee el stack real (no solo 429).
export PULSE_RATE_LIMIT_MAX=100000000
LOG_FILE="tests/gauntlet_report.txt"
exec > >(tee -a "$LOG_FILE") 2>&1

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

APP="stress_target"
DB="pulse_load_db"
REDIS="pulse_load_redis"
CLI="./target/release/pulse-cli"
export PULSE_API_URL="http://127.0.0.1:8080/api/v1"

echo "---------------------------------------------------"
echo "🔥 [LEVEL 6] SYSTEM GAUNTLET"
echo "---------------------------------------------------"

function cleanup() {
    pkill -9 -f "stress_target" || true
    docker rm -f $DB $REDIS > /dev/null 2>&1 || true
    if [ -f "pulse_core/src/lib.rs.bak" ]; then mv pulse_core/src/lib.rs.bak pulse_core/src/lib.rs; fi
    rm -rf $APP post.lua
}

trap cleanup EXIT
cleanup

# Infra
docker run --name $REDIS -p 0.0.0.0:6379:6379 -d redis:alpine > /dev/null
docker run --name $DB --shm-size=256mb -e POSTGRES_PASSWORD=s -e POSTGRES_USER=p -e POSTGRES_DB=d -p 0.0.0.0:5432:5432 -d postgres:alpine -c max_connections=200 > /dev/null
sleep 3
# [FIX] Added sleep before psql
until docker exec $DB pg_isready -U p > /dev/null 2>&1; do sleep 1; done
sleep 2
docker exec -i $DB psql -U p -d d -c "CREATE TABLE users (id UUID PRIMARY KEY, username VARCHAR, email VARCHAR, password_hash VARCHAR NOT NULL DEFAULT '', created_at TIMESTAMP);" > /dev/null
docker exec -i $DB psql -U p -d d -c "CREATE TABLE blackbox_records (id UUID PRIMARY KEY, handler VARCHAR, payload JSONB, error VARCHAR, timestamp TIMESTAMP WITH TIME ZONE);" > /dev/null

# App
rm -rf $APP
$CLI new $APP > /dev/null
echo >> $APP/Cargo.toml; echo '[workspace]' >> $APP/Cargo.toml

# Performance Patch
LIB_PATH="src/lib.rs"
if [ ! -f "$LIB_PATH" ]; then LIB_PATH="pulse_core/src/lib.rs"; fi
cp $LIB_PATH ${LIB_PATH}.bak
sed -i 's/max_connections(100)/max_connections(1000)/g' $LIB_PATH

cat << 'RUST' > $APP/src/main.rs
use pulse_core::{bootstrap, PulseConfig, utoipa::OpenApi};
use pulse_core::api::{config as api_routes, ApiDoc};
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let config = PulseConfig {
        database_url: "postgres://p:s@127.0.0.1:5432/d".to_string(),
        redis_url: Some("redis://127.0.0.1:6379".to_string()),
        host: "0.0.0.0".to_string(),
        port: 8080,
        db_max_connections: 1000
    };
    bootstrap(config, api_routes, ApiDoc::openapi()).await
}
RUST

cd $APP
cargo build --release --quiet
./target/release/$APP > /dev/null 2>&1 &
SERVER_PID=$!
cd ..
sleep 3

echo -e "${YELLOW}   -> Load Test...${NC}"
# Creamos el script de wrk ANTES del if: la rama Docker monta este archivo, así
# que debe existir (si no, Docker crea un directorio y wrk falla con "Is a directory").
# Cada request genera un usuario único con password válido (evita 409 por UNIQUE).
cat << 'LUA' > post.lua
math.randomseed(os.time() + (tonumber(tostring({}):sub(8)) or 0))
request = function()
    local n = math.random(1, 2000000000)
    local body = string.format(
        '{"username":"u%d","email":"e%d@test.com","password":"Str0ng-Pass1"}', n, n)
    return wrk.format("POST", nil, {["Content-Type"] = "application/json"}, body)
end
LUA
if command -v wrk &> /dev/null; then
    wrk -t8 -c200 -d10s -s post.lua http://127.0.0.1:8080/api/v1/users
else
    docker run --rm --net=host -v "${HOST_PWD:-$(pwd)}/post.lua:/post.lua" williamyeh/wrk -t8 -c200 -d10s -s /post.lua http://127.0.0.1:8080/api/v1/users
fi

echo -e "${YELLOW}   -> Monitor Check...${NC}"
sleep 2
timeout 5s $CLI ops:monitor || true

echo -e "${RED}   -> Chaos: Killing Redis...${NC}"
docker stop $REDIS
sleep 2
echo -e "${YELLOW}   -> Survival Check...${NC}"
if kill -0 $SERVER_PID 2>/dev/null; then
    echo -e "${GREEN}   SUCCESS${NC}"
else
    echo -e "${RED}   FAIL${NC}"
    exit 1
fi

mv ${LIB_PATH}.bak $LIB_PATH
echo -e "${GREEN}✅ PASSED${NC}"
