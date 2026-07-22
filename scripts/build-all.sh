#!/usr/bin/env bash
# Build IronClaw and all bundled channels.
#
# Run this before release or when channel sources have changed.
# The main binary bundles telegram.wasm via include_bytes!; it must exist.

set -euo pipefail

cd "$(dirname "$0")/.."

echo "Building bundled channels..."
if [ -d "channels-src/telegram" ]; then
    ./channels-src/telegram/build.sh
fi

echo ""
echo "Building IronClaw..."
cargo build --release

echo ""
echo "Building IronClaw Reborn (WebChat v2)..."
# webui-v2-beta: enables 'ironclaw-reborn serve' (HTTP gateway)
# postgres:      enables production PostgreSQL storage backend
# libsql:       enables embedded storage and the offline budget admin command
/home/deploy/.cargo/bin/cargo build --release -p ironclaw_reborn_cli     --features webui-v2-beta,postgres,libsql

echo ""
echo "Done."
echo "  ironclaw binary:        target/release/ironclaw"
echo "  ironclaw-reborn binary: target/release/ironclaw-reborn"
