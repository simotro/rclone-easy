#!/bin/sh
# Scarica il binario ufficiale di rclone (Linux x86_64) e lo posiziona come
# sidecar Tauri sotto src-tauri/binaries/, con il suffisso di target-triple
# richiesto dalla convenzione di Tauri per gli externalBin. Non è
# versionato in git (vedi src-tauri/binaries/.gitignore): va rieseguito
# dopo un clone pulito o quando si vuole aggiornare la versione inclusa.
set -eu

TARGET_TRIPLE="x86_64-unknown-linux-gnu"
DEST_DIR="$(cd "$(dirname "$0")/.." && pwd)/src-tauri/binaries"
DEST_BIN="$DEST_DIR/rclone-$TARGET_TRIPLE"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Scarico rclone (linux-amd64)..."
curl -fsSL -o "$TMP_DIR/rclone.zip" https://downloads.rclone.org/rclone-current-linux-amd64.zip

echo "Estraggo..."
unzip -q "$TMP_DIR/rclone.zip" -d "$TMP_DIR"
EXTRACTED_BIN="$(find "$TMP_DIR" -maxdepth 2 -type f -name rclone)"

mkdir -p "$DEST_DIR"
cp "$EXTRACTED_BIN" "$DEST_BIN"
chmod +x "$DEST_BIN"

echo "Installato in $DEST_BIN"
"$DEST_BIN" version | head -1
