# 打包 Windows 安装包：Tauri 会先构建页面（beforeBuildCommand），再打 NSIS 安装包，最后把安装包复制成英文文件名。
# 在项目根目录运行：
#   powershell -ExecutionPolicy Bypass -File scripts\build-app.ps1
$ErrorActionPreference = "Stop"
Set-Location (Resolve-Path (Join-Path $PSScriptRoot ".."))

foreach ($command in @("node", "pnpm", "cargo")) {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "找不到 $command，请先安装 Node.js、pnpm 和 Rust（rustup）。"
    }
}
if (-not (Test-Path "node_modules")) { pnpm install --frozen-lockfile }

pnpm tauri build --bundles nsis
if ($LASTEXITCODE -ne 0) { throw "打包失败（退出码 $LASTEXITCODE）" }

$version = (Get-Content src-tauri/tauri.conf.json | ConvertFrom-Json).version
# 按版本号找：以前打的旧版本安装包也留在这个文件夹里
$setup = Get-ChildItem "src-tauri/target/release/bundle/nsis/*_${version}_x64-setup.exe" | Select-Object -First 1
if (-not $setup) { throw "没找到 $version 版的安装包" }
New-Item -ItemType Directory -Force out | Out-Null
Copy-Item $setup.FullName "out/PluginHub_${version}_x64-setup.exe" -Force
Write-Host "安装包：out\PluginHub_${version}_x64-setup.exe"
