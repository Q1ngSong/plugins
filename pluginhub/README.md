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
  claude.rs, codex.rs   两边的适配：状态、链接、卸载、真实检查（Codex 还看技能清单超没超上限）
  skills.rs             独立的技能：扫描两边的 skills 文件夹、统一存放、链接、删除、检查
  gitx.rs               git：克隆、拉取、本地修改检测、读清单、备份
  source.rs             添加框里粘进来的东西怎么认：GitHub 的各种写法、子目录和技能的链接、npx skills add 命令（只认格式，不联网）
  usage.rs              扫描两边的会话记录，统计哪些项目用过插件
  schedule.rs           后台任务：设置、状态、常驻后台进程的锁；系统任务怎么建交给 platform/
  server.rs             浏览器版的页面服务：只听 127.0.0.1，校验 Host 和令牌
  util.rs               路径、日志、JSON、目录链接、子进程、锁（平台无关）
  platform/             平台层：和系统打交道的都在这里，别的模块不写 #[cfg]
    mod.rs              两个实现共同提供的函数清单，按编译目标选一个
    windows.rs          junction、计划任务和「启动」文件夹、不弹黑框、OEM 解码、找 claude.exe 和 codex.exe
    macos.rs            符号链接、launchd、登录 shell 的 PATH、open、找 claude 和 codex
src-tauri/tauri.conf.json         Tauri 的公共设置
src-tauri/tauri.windows.conf.json 只在 Windows 上合并进来：NSIS 安装包
src-tauri/tauri.macos.conf.json   只在 macOS 上合并进来：.app 和 .dmg、临时签名
scripts/build-app.ps1   Windows 本机打安装包
scripts/build-app.sh    macOS 本机打 .app 和 .dmg
scripts/e2e.py          端到端测试
```

## 构建

需要 Node.js、pnpm 和 Rust（版本见 `rust-toolchain.toml`）。Windows 和 macOS 都能构建，命令一样：

```sh
pnpm install
pnpm build:renderer
cd src-tauri && cargo build --release
```

页面构建到 `dist/`，打包时嵌进程序；程序在 `src-tauri/target/release/pluginhub`（Windows 上是 `pluginhub.exe`）。

打安装包：

```powershell
# Windows：NSIS 安装包，复制到 out/PluginHub_<版本>_x64-setup.exe
powershell -ExecutionPolicy Bypass -File scripts\build-app.ps1
```

```sh
# macOS：.app 在 src-tauri/target/release/bundle/macos/插件中心.app，dmg 复制到 out/PluginHub_<版本>_<arch>.dmg
bash scripts/build-app.sh
```

改了后端要重新构建程序。构建前先停掉在跑的 `pluginhub --run daemon`（Windows 上它占着 exe 文件）。

macOS 的 .app 只做了临时签名（ad-hoc），本机构建的能直接打开；发给别人要过 Gatekeeper，见仓库首页 README 的安装一节。

## 平台相关的代码

和操作系统打交道的代码只在 `src-tauri/src/platform/` 里：`windows.rs` 和 `macos.rs` 提供同一套函数（清单在 `mod.rs` 开头），`mod.rs` 按编译目标选一个，其余模块只写 `platform::xxx`，不写 `#[cfg]`。要支持新系统（比如 Linux），照着 `macos.rs` 写一个 `linux.rs`（符号链接和 PATH 的做法一样，`open` 换成 `xdg-open`，launchd 换成 systemd 的用户定时器），在 `mod.rs` 里加两行，再加一个 `tauri.linux.conf.json` 和 CI 里的一个 job 就够了。

## 测试

单元测试覆盖读 SKILL.md 开头的解析、技能开销的估算、读 Codex 模型提示里的技能清单（缩写的路径、被截短和只剩名字的说明）、添加框输入的识别（GitHub 的各种写法、`npx skills add` 命令）、插件清单的判断，以及会话记录里从仓库装的技能怎么计数：

```powershell
cd src-tauri; cargo test
```

`scripts/e2e.py` 是端到端测试，要本机有 Python，Windows 和 macOS 都能跑。它用一个本地 git 仓库当远端，通过 `pluginhub --run api` 调接口，真的往 Claude Code 和 Codex 里装一个叫 yp-e2e 的测试插件，走一遍添加、锁定、本地修改、另存并还原、后台检查修复配置、真实检查、卸载。真实检查有没有顺带记下 Codex 的技能清单，单独重新检查能不能用，也一并看。再在 Codex 里放一个叫 yp-e2e-skill 的测试技能，走一遍装到 Claude Code、链接被删后修复、从一边删、从所有 app 删。然后用一个只有技能的本地仓库，走一遍 `npx skills add` 命令识别、整条命令直接交给 add 装技能、远端更新后同步、`~/.agents/skills` 里有同名技能时不往 Codex 重复装、同一仓库再装一个、仓库后来有了插件清单时同一份克隆再装成插件、卸掉插件时技能还在用就留着克隆、技能删光后仓库一起不再管理。最后清理干净，并核对 Codex 的 config.toml 和 Claude Code 的 settings.json 都和测试前一样。

```powershell
python scripts\e2e.py
```

```sh
python3 scripts/e2e.py   # macOS；程序不在默认位置时用环境变量 PLUGINHUB_EXE 指定
```

## 发布

版本号在四个地方：`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`，还有 `src-tauri/src/util.rs` 里的 `HUB_VERSION`。

打一个 `v1.2.0` 这样的 tag 推到 GitHub，Actions（`.github/workflows/release.yml`）会在 Windows 和 macOS 上各构建一份（NSIS 安装包、通用的 dmg），改成英文文件名，建一个草稿 Release。检查过后，在网页上点 Publish。

## 插图

根目录 README 的插图在 `assets/`，都是 SVG 文件，改文字可以直接改文件里的 `<text>`。

## 主页

[项目主页](https://q1ngsong.github.io/plugins/)是根目录的 `index.html`，由 `site/build.py` 把模板和根目录 README 拼出来。改了 README 之后运行 `python3 site/build.py` 重新生成，和 README 一起提交。详见 [site/README.md](../site/README.md)。
