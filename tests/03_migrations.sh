#!/bin/bash
set -e
GREEN='\033[0;32m'
NC='\033[0m'
export DATABASE_URL="postgres://pulse:secret@localhost:5433/pulse_mig_test"

echo "---------------------------------------------------"
echo "🗄️  [LEVEL 3] MIGRATIONS"
echo "---------------------------------------------------"

docker rm -f pg_mig > /dev/null 2>&1 || true
# [FIX] Added shm-size
docker run --name pg_mig --shm-size=256mb -e POSTGRES_PASSWORD=secret -e POSTGRES_USER=pulse -e POSTGRES_DB=pulse_mig_test -p 5433:5432 -d postgres:alpine > /dev/null
echo -n "   -> Waiting DB..."
until docker exec pg_mig pg_isready -U pulse > /dev/null 2>&1; do sleep 1; done
# [FIX] Stabilization sleep
sleep 2
echo " OK."

echo -n "   -> Migrating UP..."
cargo run -p migration --quiet -- up > /dev/null
echo -e "${GREEN} OK${NC}"

echo -n "   -> Migrating DOWN..."
cargo run -p migration --quiet -- down > /dev/null
echo -e "${GREEN} OK${NC}"

docker rm -f pg_mig > /dev/null
echo -e "${GREEN}✅ PASSED${NC}"
