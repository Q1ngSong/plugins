# 插件中心：开发说明

用法见[仓库首页的 README](../README.md)。这里讲代码在哪、怎么构建、测试和发布。

## 代码

```
src/                    页面（React + Vite + Tailwind，样式照 AgentPulse）
  lib/api.ts            接口层：窗口里走 Tauri 的 invoke("api")，浏览器里走 fetch 加令牌
src-tauri/src/          程序（Rust，Tauri 2）
  main.rs               入口：开窗口、页面调用的 api 命令；--run 转到命令行
  cli.rs                命令行：status、check、update、add、guard、auto、daemon、install、uninstall、serve、api
  ops.rs                各项操作和接口分发
  view.rs               状态汇总、移植 skill
  claude.rs, codex.rs   两边的适配：状态、链接、卸载、真实检查
  gitx.rs               git：克隆、拉取、本地修改检测、读清单、备份
  usage.rs              扫描两边的会话记录，统计哪些项目用过插件
  schedule.rs           后台任务：计划任务，建不了就用「启动」文件夹加常驻进程
  server.rs             浏览器版的页面服务：只听 127.0.0.1，校验 Host 和令牌
  util.rs               路径、日志、JSON、目录链接、子进程、锁
scripts/build-app.ps1   本机打安装包
scripts/e2e.py          端到端测试
```

## 构建

需要 Node.js、pnpm 和 Rust（版本见 `rust-toolchain.toml`）。

```powershell
pnpm install
pnpm build:renderer
cd src-tauri; cargo build --release
```

页面构建到 `dist/`，打包时嵌进 exe；程序在 `src-tauri/target/release/pluginhub.exe`。

打安装包：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-app.ps1
```

安装包在 `out/PluginHub_<版本>_x64-setup.exe`。

改了后端要重新构建 exe。构建前先停掉在跑的 `pluginhub.exe --run daemon`，它占着 exe 文件。

## 测试

没有单元测试。`scripts/e2e.py` 是端到端测试，要本机有 Python。它用一个本地 git 仓库当远端，通过 `pluginhub.exe --run api` 调接口，真的往 Claude Code 和 Codex 里装一个叫 yp-e2e 的测试插件，走一遍添加、锁定、本地修改、另存并还原、后台检查修复配置、真实检查、卸载，最后清理干净。

```powershell
python scripts\e2e.py
```

## 发布

版本号在四个地方：`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`，还有 `src-tauri/src/util.rs` 里的 `HUB_VERSION`。

打一个 `v1.2.0` 这样的 tag 推到 GitHub，Actions（`.github/workflows/release.yml`）会构建安装包，改成英文文件名，建一个草稿 Release。检查过后，在网页上点 Publish。

## 插图

根目录 README 的插图在 `assets/`，都是 SVG 文件，改文字可以直接改文件里的 `<text>`。
