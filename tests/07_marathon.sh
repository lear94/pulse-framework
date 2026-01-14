#!/bin/bash
set -e
LOG_FILE="tests/marathon_report.txt"
exec > >(tee -a "$LOG_FILE") 2>&1

GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[1;33m'
NC='\033[0m'

APP="marathon_target"
DB="pulse_mara_db"
REDIS="pulse_mara_redis"
CLI="./target/release/pulse-cli"

echo "---------------------------------------------------"
echo "🏃 [LEVEL 7] THE MARATHON (1 Million Requests)"
echo "---------------------------------------------------"

function cleanup() {
    pkill -9 -f "$APP" || true
    docker rm -f $DB $REDIS > /dev/null 2>&1 || true
    if [ -f "pulse_core/src/lib.rs.bak" ]; then mv pulse_core/src/lib.rs.bak pulse_core/src/lib.rs; fi
    rm -rf $APP
}

trap cleanup EXIT
cleanup

echo -e "${CYAN}   -> Deploying Heavy Infra...${NC}"
docker run --name $REDIS -p 0.0.0.0:6379:6379 -d redis:alpine > /dev/null
# [FIX] Massive shm-size and tuned postgres
docker run --name $DB --shm-size=512mb -e POSTGRES_PASSWORD=s -e POSTGRES_USER=p -e POSTGRES_DB=d -p 0.0.0.0:5432:5432 -d postgres:alpine -c max_connections=500 -c synchronous_commit=off > /dev/null

sleep 5
# [FIX] Stabilization
until docker exec $DB pg_isready -U p > /dev/null 2>&1; do sleep 1; done
sleep 2
docker exec -i $DB psql -U p -d d -c "CREATE TABLE users (id UUID PRIMARY KEY, username VARCHAR, email VARCHAR, created_at TIMESTAMP);" > /dev/null
docker exec -i $DB psql -U p -d d -c "CREATE TABLE blackbox_records (id UUID PRIMARY KEY, handler VARCHAR, payload JSONB, error VARCHAR, timestamp TIMESTAMP WITH TIME ZONE);" > /dev/null

# Config
rm -rf $APP
$CLI new $APP > /dev/null
echo >> $APP/Cargo.toml; echo '[workspace]' >> $APP/Cargo.toml

LIB_PATH="src/lib.rs"
if [ ! -f "$LIB_PATH" ]; then LIB_PATH="pulse_core/src/lib.rs"; fi
cp $LIB_PATH ${LIB_PATH}.bak
sed -i 's/max_connections(100)/max_connections(2000)/g' $LIB_PATH

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
        db_max_connections: 2000
    };
    bootstrap(config, api_routes, ApiDoc::openapi()).await
}
RUST

echo -e "${CYAN}   -> Compiling & Starting Server...${NC}"
cd $APP
cargo build --release --quiet
./target/release/$APP > /dev/null 2>&1 &
SERVER_PID=$!
cd ..
sleep 5

echo -e "${YELLOW}   -> STARTING 1 MILLION REQUEST ATTACK (Duration: 300s)...${NC}"
if command -v wrk &> /dev/null; then
    echo 'wrk.method="POST"; wrk.body="{\"username\":\"u\",\"email\":\"e@t.com\"}"; wrk.headers["Content-Type"]="application/json"' > post.lua
    wrk -t12 -c400 -d300s -s post.lua http://127.0.0.1:8080/api/v1/users
else
    docker run --rm --net=host -v $(pwd)/post.lua:/post.lua williamyeh/wrk -t12 -c400 -d300s -s /post.lua http://127.0.0.1:8080/api/v1/users
fi

echo -e "${CYAN}   -> Verifying Data Integrity...${NC}"
COUNT=$(docker exec $DB psql -U p -d d -t -c "SELECT count(*) FROM users;" | tr -d ' ')
echo -e "      Records Inserted: ${GREEN}$COUNT${NC}"

if [ "$COUNT" -ge 1000000 ]; then
    echo -e "${GREEN}✅ MARATHON PASSED${NC}"
else
    echo -e "${YELLOW}⚠️  FINISHED (Count: $COUNT)${NC}"
fi

mv ${LIB_PATH}.bak $LIB_PATH
