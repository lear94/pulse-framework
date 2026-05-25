#!/bin/bash
set -e
GREEN='\033[0;32m'
CYAN='\033[0;36m'
NC='\033[0m'

echo -e "${CYAN}🏆 PULSE FRAMEWORK CERTIFICATION SUITE${NC}"
echo "==================================================="

echo -e "\n${CYAN}[1/9] LOGICAL INTEGRITY${NC}"
cargo test --quiet
echo -e "${GREEN}>>> Passed.${NC}"

./tests/02_cli.sh
./tests/03_migrations.sh
./tests/04_security.sh
./tests/05_recovery.sh
./tests/06_gauntlet.sh
./tests/07_idempotency.sh
./tests/08_toxic.sh

echo ""
echo -e "${CYAN}[9/9] THE MARATHON (Optional - 5 min)${NC}"
# No-interactivo (Docker/CI): se salta salvo que pidas RUN_MARATHON=1.
# Interactivo: pregunta como siempre.
if [ -t 0 ]; then
    read -p "Run the 1 Million Request Marathon? (y/n) " -n 1 -r
    echo ""
elif [ "${RUN_MARATHON:-0}" = "1" ]; then
    REPLY=y
else
    REPLY=n
    echo "   (non-interactive: skipping marathon; set RUN_MARATHON=1 to force)"
fi
if [[ $REPLY =~ ^[Yy]$ ]]; then
    ./tests/07_marathon.sh
fi

echo ""
echo "==================================================="
echo -e "${GREEN}⭐ READY FOR PRODUCTION ⭐${NC}"
echo "==================================================="

rm -rf stress_target security_target recovery_target marathon_target pulse_cli_test idempotency_target toxic_target storage/blackbox.jsonl tests/*.txt post.lua toxic_payload.json toxic_blob.json large_blob.txt
echo "🧹 Cleaned."
