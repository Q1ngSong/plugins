#!/bin/bash
# 打包 macOS 应用：Tauri 先构建页面（beforeBuildCommand），再打 .app 和 .dmg，最后把 dmg 复制成英文文件名。
# 在 pluginhub 目录下运行：
#   bash scripts/build-app.sh          # .app 和 .dmg
#   bash scripts/build-app.sh app      # 只要 .app（不做 dmg，快一些）
set -euo pipefail
cd "$(dirname "$0")/.."

for command in node pnpm cargo; do
  command -v "$command" >/dev/null 2>&1 || { echo "找不到 $command，请先安装 Node.js、pnpm 和 Rust（rustup）。" >&2; exit 1; }
done
[ -d node_modules ] || pnpm install --frozen-lockfile

bundles="${1:-app,dmg}"
pnpm tauri build --bundles "$bundles"

version=$(node -p "require('./src-tauri/tauri.conf.json').version")
echo "应用：src-tauri/target/release/bundle/macos/插件中心.app"
if [[ "$bundles" == *dmg* ]]; then
  arch=$(uname -m) # arm64 或 x86_64
  mkdir -p out
  # 按版本号找：以前打的旧版本也留在这个文件夹里
  dmg=$(ls src-tauri/target/release/bundle/dmg/*_"${version}"_*.dmg | head -n 1)
  cp "$dmg" "out/PluginHub_${version}_${arch}.dmg"
  echo "安装包：out/PluginHub_${version}_${arch}.dmg"
fi
