#!/usr/bin/env bash
# Levanta la Pulse Wiki de demo: Postgres en un contenedor + la app compilada y
# ejecutada dentro de la imagen `pulse-test` (que ya trae Rust + openssl, igual
# que la suite de certificación). Pensado para entornos donde el host no tiene
# las cabeceras de openssl (WSL/Docker Desktop): todo ocurre en contenedores.
#
# Uso:   ./run.sh           # arranca (Ctrl-C para parar; limpia el Postgres)
#        KEEP_DB=1 ./run.sh # no borra el contenedor de Postgres al salir
set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(cd ../.. && pwd)"          # raíz del repo (crate pulse_core)

IMAGE="pulse-test"
DB="wiki_demo_db"
DB_PORT=5440

cleanup() {
    if [ "${KEEP_DB:-0}" != "1" ]; then
        docker rm -f "$DB" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# 1) Imagen con el toolchain (la misma de la suite). Se construye si falta.
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo ">> Building toolchain image ($IMAGE)…"
    docker build -f "$ROOT/Dockerfile.test" -t "$IMAGE" "$ROOT"
fi

# 2) Postgres.
echo ">> Starting Postgres ($DB) on :$DB_PORT…"
docker rm -f "$DB" >/dev/null 2>&1 || true
docker run --name "$DB" \
    -e POSTGRES_USER=wiki -e POSTGRES_PASSWORD=wiki -e POSTGRES_DB=wiki \
    -p "${DB_PORT}:5432" -d postgres:alpine >/dev/null
until docker exec "$DB" pg_isready -U wiki >/dev/null 2>&1; do sleep 1; done
sleep 1
echo ">> Postgres ready."

# 3) Build + run de la wiki dentro del contenedor (network host: alcanza el
#    Postgres en 127.0.0.1:$DB_PORT y publica el server en :8080 del host).
echo ">> Building & launching the wiki (first build downloads/compiles deps)…"
echo ">> Open http://localhost:8080  ·  login: admin / Str0ng-Pass1"
exec docker run --rm -it --network host \
    -e DATABASE_URL="postgres://wiki:wiki@127.0.0.1:${DB_PORT}/wiki" \
    -e JWT_SECRET="pulse-wiki-demo-secret-key-change-me-please" \
    -e PULSE_ADMIN_USERS="admin" \
    -e PULSE_RATE_LIMIT_MAX="1000" \
    -e RUST_LOG="info" \
    -v "$ROOT":/app \
    -w /app/examples/wiki \
    -v pulse-cargo-registry:/usr/local/cargo/registry \
    -v pulse-wiki-target:/app/examples/wiki/target \
    "$IMAGE" cargo run --release
