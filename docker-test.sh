#!/usr/bin/env bash
# Ejecuta la suite completa de certificación (run_tests.sh) dentro de un
# contenedor con todo el toolchain + openssl, sin ensuciar tu máquina.
#
# Por qué estos flags:
#  --network host          : los scripts arrancan Postgres/Redis con `docker run`
#                            y el server/CLI los alcanzan por 127.0.0.1:<puerto>.
#  -v docker.sock          : docker-in-docker — los `docker run` internos crean
#                            contenedores hermanos en el daemon del host.
#  -v $PWD:/app            : monta el código (build incremental, sin rebuild de imagen).
#  -v pulse-cargo-*        : cachea registry y target entre corridas (mucho más rápido).
#
# Requisitos en el host: Docker en marcha. (En Docker Desktop/WSL el socket está
# en /var/run/docker.sock.)
set -euo pipefail
cd "$(dirname "$0")"

IMAGE="pulse-test"

echo ">> Building test harness image ($IMAGE)..."
docker build -f Dockerfile.test -t "$IMAGE" .

echo ">> Running certification suite inside container..."
exec docker run --rm -it \
    --network host \
    -v /var/run/docker.sock:/var/run/docker.sock \
    -e HOST_PWD="$PWD" \
    -v "$PWD":/app \
    -v pulse-cargo-registry:/usr/local/cargo/registry \
    -v pulse-cargo-target:/app/target \
    "$IMAGE" "$@"
