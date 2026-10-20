#!/usr/bin/env bash
# 用法：scripts/build-linux.sh [amd64] [arm64]；不带参数时两种架构都构建。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ARCHES=("$@")
[ ${#ARCHES[@]} -gt 0 ] || ARCHES=(amd64 arm64)
triple() {
  case "$1" in
    amd64) echo x86_64-unknown-linux-gnu ;;
    arm64) echo aarch64-unknown-linux-gnu ;;
    *) echo "不支持的架构：$1（可选 amd64、arm64）" >&2; return 2 ;;
  esac
}
for ARCH in "${ARCHES[@]}"; do triple "$ARCH" >/dev/null; done
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend test
pnpm --dir frontend build
VERSION="$(node -p "require('./plugin.json').version")"
mkdir -p .build/cargo
for ARCH in "${ARCHES[@]}"; do
  TRIPLE="$(triple "$ARCH")"
  # Rust 产物按架构分目录，避免交替构建时互相覆盖缓存。
  TARGET_DIR=".build/linux-target-$ARCH"
  docker run --rm --platform "linux/$ARCH" \
    -v "$ROOT:/work" -v "${LINUX_CARGO_CACHE:-$ROOT/.build/cargo}:/cache" \
    -e CARGO_HOME=/cache -e CARGO_TARGET_DIR="/work/$TARGET_DIR" -w /work rust:1.99.1-slim-bookworm \
    sh -c 'cargo test --locked --manifest-path backend/Cargo.toml && cargo build --locked --release --manifest-path backend/Cargo.toml'
  "${CPR_PLUGIN_BIN:-cpr-plugin}" package --manifest "$ROOT/plugin.json" \
    --binary "$ROOT/$TARGET_DIR/release/codex-model-health-plugin" \
    --target "$TRIPLE" --resource-map web=frontend/dist \
    --resource-map legal=. --output-dir "$ROOT/dist"
  python3 scripts/package-smoke.py "$ROOT/dist/local.model-health-$VERSION-$TRIPLE.tar.gz"
done
