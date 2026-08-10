#!/bin/sh
# Compila e pacchettizza Rclone Easy per Linux dentro un container Docker
# (Ubuntu 22.04, vedi docker/build-linux.Dockerfile per il perché), senza
# passare da GitHub Actions — utile su un rolling release come CachyOS dove
# la build locale nativa dell'AppImage fallisce (strip di linuxdeploy
# troppo vecchio per il toolchain di sistema).
#
# Cache dentro volumi Docker dedicati (registro Cargo e node_modules): solo
# la primissima esecuzione è lenta, quelle successive sono incrementali.
# Il target di build usato qui (CARGO_TARGET_DIR=src-tauri/target-docker)
# è volutamente SEPARATO da src-tauri/target/ (quello di `npm run tauri
# dev`/build nativa) — mischiare artefatti compilati dentro il container
# con quelli compilati nativamente sull'host può confondere Cargo (stesso
# target-triple, ambienti diversi).
set -eu

cd "$(dirname "$0")/.."

IMAGE_TAG="rclone-easy-linux-builder"

echo "Costruisco l'immagine (salta se già aggiornata)..."
docker build -f docker/build-linux.Dockerfile -t "$IMAGE_TAG" docker/

if [ ! -f src-tauri/binaries/rclone-x86_64-unknown-linux-gnu ]; then
    echo "Scarico il sidecar rclone..."
    ./scripts/fetch-rclone-sidecar.sh
fi

echo "Build dentro il container..."
docker run --rm \
    -v "$PWD:/workspace" \
    -v rclone-easy-cargo-registry:/usr/local/cargo/registry \
    -v rclone-easy-node-modules:/workspace/node_modules \
    -e CARGO_TARGET_DIR=/workspace/src-tauri/target-docker \
    -w /workspace \
    "$IMAGE_TAG" \
    sh -c "npm ci && npm run tauri build"

echo "Ripristino i permessi dei file generati..."
docker run --rm -v "$PWD:/workspace" "$IMAGE_TAG" chown -R "$(id -u):$(id -g)" /workspace/src-tauri/target-docker

echo ""
echo "Fatto. Pacchetti in src-tauri/target-docker/release/bundle/"
