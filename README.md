# 插件中心

让 Claude Code 和 Codex 用同一份插件和技能。

Windows 和 macOS 桌面程序，安装包只有几 MB，下载就能用，不需要 Python。

![插件只在 ~/.yuwanplugins 里存一份，Claude Code 和 Codex 都用目录链接指向它](assets/hero.svg)

## 起因

我写了一个插件，叫 [design-code-like-a-human](https://github.com/Q1ngSong/design-code-like-a-human)。同一份代码，同时装在 Claude Code 和 Codex 里。用了一阵，两边开始各过各的日子。

Claude Code 那边一直停在 9 月 5 日的版本。仓库里明明有新提交，`claude plugin update` 每次都回答已经是最新。后来翻了它的源码才明白：它只认 `plugin.json` 里的版本号，而那个数字我一直没动过。

Codex 没有更新命令，我只好手动把插件文件夹整个换掉，换之前先备份一份。等回过头看，`~/plugins` 下面已经躺着 11 个 `.backup-` 开头的文件夹。

最难察觉的是开关。切换 API 服务商的工具会把 `~/.claude/settings.json` 整份重写，插件的启用状态就这么没了，不报错，也没有任何提示。

这三件事说到底是一回事：一个插件在电脑上存了好几份，每份归不同的程序管。插件中心的办法是只留一份，放在 `~/.yuwanplugins`，两个 app 都用目录链接指过来。更新就是在这一份里 `git pull`，两边一起生效。

## 安装

### Windows

到 [Releases](../../releases) 下载 `PluginHub_<版本>_x64-setup.exe`，双击安装。装在当前用户目录下，用不着管理员权限。

电脑上要有 [Git for Windows](https://git-scm.com/download/win)，Claude Code 和 Codex 至少装一个。WebView2 是 Windows 10 和 11 自带的，万一缺了，安装程序会自己补上。

安装包没有代码签名，第一次运行时 SmartScreen 会拦一下，点「更多信息」，再点「仍要运行」。

### macOS

到 [Releases](../../releases) 下载 `PluginHub_<版本>_universal.dmg`，打开后把「插件中心」拖进「应用程序」。Apple 芯片和 Intel 的 Mac 都能用，要 macOS 10.15 以上。

电脑上要有 git（装过 Xcode 命令行工具或 Homebrew 的都有），Claude Code 和 Codex 至少装一个。两边的命令行放在哪都认得：Claude Code 桌面版自带的、官方脚本装到 `~/.local/bin` 的、Homebrew 或 npm 装的；Codex 桌面版和 ChatGPT 桌面版自带的、npm 装的。

应用没有签名和公证，第一次打开时 macOS 会说无法验证开发者：在访达里右键点它，选「打开」；或者在终端运行 `xattr -dr com.apple.quarantine /Applications/插件中心.app`。

## 用起来

### 打开就能看到全部

![首页：本机所有插件排成一面卡片墙，标出装在哪个 app、有没有新版本](assets/home.svg)

首页是一面卡片墙。本机装的插件和技能都在这里，Claude Code 的和 Codex 的放在一起，同名的合成一张。卡片底部的小色块说明它装在哪边：橙色是 Claude Code，绿色是 Codex。顶上可以只看插件或只看技能，也可以只看某一个 app 里的。

右上角三个按钮：删除、同步、打开仓库。只装在一边的插件会多一个安装按钮，用来装到另一边。有新版本、被锁定、文件夹里有改动、检查没通过，卡片上都会直接标出来。

点开卡片是详情页：每个 app 里装的是哪个版本，最近一次检查的结果，还有哪些项目用过它。项目是从两边的会话记录里数出来的。

### 加插件时不用填版本号

![添加插件：填仓库地址，列出各分支的版本和最新提交，选一个分支和要装的 app](assets/add.svg)

贴一个仓库地址，点查询。插件中心会把每个分支最新的那次提交拉下来看一眼，只读清单文件，不下载大文件。每个分支上插件是什么版本、最后一次提交写了什么、支持哪个 app，都列在那里。

你要做的只是挑一个分支，勾上要装的 app。以后它就跟着这个分支走。装完会马上做一次真实检查，后面会讲这是什么。

### 只装在一边的插件

![移植：把只为 Codex 写的插件的 skill 复制出来，补上清单，再链接进 Claude Code](assets/install.svg)

插件自带两边的清单，就直接链接过去。只为一个 app 写的，插件中心会把它的 skill 复制一份到 `~/.yuwanplugins`，补上另一边的清单，再链接进去。

skill 的格式两边通用，钩子、MCP 服务和 App 集成不是，所以这些不会带过去。有的插件全靠 MCP 或钩子干活，一个 skill 也没有，这种装不过去，页面上会写明原因。原插件更新以后，点一下同步就会重新复制。

### 独立的技能

![只装在 Codex 里的技能，装到 Claude Code 时先挪进 ~/.yuwanplugins/skills 统一存放，两边都换成链接](assets/skill.svg)

不属于任何插件、单独放在 skills 文件夹里的 skill，这里叫技能。Claude Code 读 `~/.claude/skills`，Codex 读 `~/.codex/skills`。技能比插件简单得多：没有清单，不用登记，一个带 SKILL.md 的文件夹就是全部。所以插件中心那套做法搬过来，几乎不用改。

只装在一边的技能，点安装就能装到另一边。插件中心先把它挪进 `~/.yuwanplugins/skills` 统一存放，原来的位置换成链接，再给另一个 app 也放一个链接。之后两边读的是同一份，改一处两边生效。链接被删了，后台检查会补回来。

两个 app 都没有单个技能的开关，放进去就生效。所以从某个 app 删掉技能，就是拆掉那边的链接；如果那里是个真实的文件夹，会挪进备份，不直接删。Codex 自带的技能归 Codex 自己管，这里只能看。

真实检查也照做。Codex 那边看模型提示里有没有它、读的是不是统一存放的那份；Claude Code 没有列出技能的命令，只能核对链接和 SKILL.md。哪些项目用过它、用了几次，照样从会话记录里数出来。

### 更新是一次 git pull

![以前每个 app 一份副本，各更各的；现在 git pull 一次，两边都读到新文件](assets/sync.svg)

两个 app 链接的是同一个文件夹，所以更新就是在这个文件夹里拉一次代码。不复制，不重装，也不看版本号。上游改写了历史（force push），照样跟得上。

拉取之后，开着的对话下次读 SKILL.md 就是新内容。新增或删掉 skill、改了描述或钩子，要新开一个对话才看得到；Claude Code 里也可以用 `/reload-plugins`。

想省事的话，在设置里打开自动更新，它会按间隔在后台拉取。

### 在插件文件夹里改了东西

![插件文件夹里有修改时，同步停止拉取，可以锁定保留修改，也可以另存一份再还原](assets/lock.svg)

`~/.yuwanplugins` 里的插件是给 app 读的，最好别在里面改。可人总会手痒，改一行看看效果，顺手再提交一下。

插件中心认得出这些改动：没提交的文件、本地提交、切到了别的分支。一旦发现，同步就不再拉取，免得把你的修改冲掉，然后请你二选一。

**锁定，保留修改。** 从此不检查、不同步，同步按钮变灰。想恢复，再点一下详情页右上角的锁。

**另存并还原。** 整个文件夹连同 `.git` 存进 `~/.pluginhub/saved`，本地提交也在里面。插件文件夹回到仓库里的样子，继续跟着分支更新。

锁定只管拉取。链接或配置被别的程序改坏了，后台检查照样会修。

### 它真的加载了吗

![真实检查：Claude Code 跑 plugin list 和 plugin details，Codex 跑 plugin list 和 debug prompt-input](assets/verify.svg)

配置文件写对了，不代表 app 真的把插件加载了进去。插件中心的办法是直接去问 app。

Claude Code 那边，跑 `claude plugin list` 和 `claude plugin details`，看插件在不在、开没开，再把它列出的 skill 跟插件里的逐个对上。Codex 那边，除了 `codex plugin list`，还让它把真正交给模型的提示渲染出来（`codex debug prompt-input`），在里面一个个找 skill，并确认每个 SKILL.md 的路径都在 `~/.yuwanplugins` 里，读的不是缓存里的旧副本。

装完、检查更新、后台检查时都会跑一遍。详情页里每个 app 的卡片上也能随时点「现在检查」。没通过的插件，首页卡片上会标「要检查」。

Codex 那边还顺带看一件事。它给技能清单留的地方只有上下文的 2%，超了也不报错，先截短每条说明，再只留名字，最后把排在后面的整条拿掉。模型是看着说明挑技能的，说明一短就挑不准。插件中心把提示里的每条说明和 SKILL.md 原文比一遍，发现被截短，就在首页提醒你，并列出 Codex 里占得多的几个，删掉用不上的就好。每个插件和技能每次会话大约占多少，详情页里写着。Claude Code 也有类似的上限，是上下文的 1%，可它没有渲染提示的命令，插件中心查不了，要看就在 Claude Code 里跑 `/doctor`。

### 配置被别的程序改掉

![后台检查：别的程序改掉配置后，每天一次的检查发现问题、修好，并在日志和详情页里写明](assets/guard.svg)

这个功能是被前面那个切换服务商的工具逼出来的。在设置里打开「后台检查」，插件中心每天把两边的插件配置过一遍：`settings.json` 和 `config.toml` 有没有被重写，Codex 的个人插件源有没有被改，链接还在不在。

发现问题就修好，把是哪个文件、什么时候改的记进日志，再做一次真实检查。详情页里对应的 app 卡片上也会写一句：什么时候发现了什么，修好了没有。

后台任务在 Windows 上用计划任务，没有权限建计划任务的机器上改成开机自启的后台进程；在 macOS 上用 launchd 的用户级任务（`~/Library/LaunchAgents/com.yuwan.pluginhub.plist`），登录后和每隔一段时间各跑一次。跑的都是装好的那个程序。

### 两个删除按钮

![首页卡片上的删除从所有 app 删掉；详情页里 app 卡片上的删除只动那一个 app](assets/delete.svg)

首页卡片上的删除，会把插件从所有 app 里删掉；详情页里每个 app 卡片右上角的删除，只动那一个 app。

删除只去掉链接和登记，不碰文件。插件中心不再管理的插件，文件夹挪进 `~/.pluginhub/backups`，每个插件留最近 3 份。

### 命令行

![命令行：pluginhub.exe --run status 打印每个插件在两边的状态](assets/cli.svg)

同一个程序加上 `--run` 就是命令行，后台任务用的也是它。Windows 上装好后它在 `%LOCALAPPDATA%\插件中心\pluginhub.exe`；macOS 上在 `/Applications/插件中心.app/Contents/MacOS/pluginhub`，嫌长可以在 `~/.zshrc` 里加一行 `alias pluginhub='/Applications/插件中心.app/Contents/MacOS/pluginhub'`。

| 命令 | 做什么 |
|---|---|
| `--run status` | 打印状态 |
| `--run check` | 检查有没有新版本（锁定的跳过），再做一遍真实检查 |
| `--run update [插件] [--force]` | 拉取最新，同步到两边 |
| `--run guard` | 马上做一次后台检查 |
| `--run add <仓库> [--branch B]` | 接管一个插件，不写分支就用默认分支 |
| `--run install [--interval 分钟]` | 打开自动更新 |
| `--run uninstall` | 关掉后台检查、自动更新和后台任务 |
| `--run serve [--port N] [--no-browser]` | 在浏览器里打开管理页面 |
| `--run api <接口> [JSON]` | 直接调用页面用的接口，输出 JSON |

Windows 上它是个窗口程序，cmd 不会等它跑完。要看输出，在 PowerShell 里末尾加 `| Out-Host`，或者用 `cmd /c start /wait`。macOS 的终端里直接运行就行。

## 背后的做法

插件中心靠的是目录链接：Windows 用 junction，普通用户就能建；macOS 用符号链接。删掉链接不会动到它指向的文件夹。

Claude Code 会把 `~/.claude/skills` 下带 `.claude-plugin/plugin.json` 的文件夹就地加载成 `<名字>@skills-dir`，不往缓存里复制。所以链接一放，它读的就是 `~/.yuwanplugins` 里的那份。

Codex 麻烦一些。它只认自己缓存里的真实文件夹，整个缓存目录换成链接，会被它悄悄忽略。插件中心先用 `codex plugin add` 正常装一份，再把缓存里的 `skills/`、`hooks/` 这些子文件夹逐个换成链接，清单文件还是复制的。Codex 顺着子文件夹的链接读文件，改动就能直接生效。插件的版本号变了，缓存目录的名字跟着变，插件中心会重新装一次。

技能省事得多。两个 app 都认链接过来的技能文件夹，Codex 报告的也是链接指向的真实路径，所以在两边的 skills 文件夹里各放一个链接就够了。

## 文件放在哪

| 位置 | 内容 |
|---|---|
| `~/.yuwanplugins/<名字>` | 插件本体，一份 git 克隆，两边都链接到这里 |
| `~/.yuwanplugins/skills/<名字>` | 统一存放的技能，两边都链接到这里 |
| `~/.pluginhub/config.json` | 管理的插件和设置 |
| `~/.pluginhub/hub.log` | 日志 |
| `~/.pluginhub/usage.json` | 会话记录的扫描缓存，删了会重扫 |
| `~/.pluginhub/backups` | 不再管理的插件和以前复制出来的旧文件夹，每个插件留 3 份 |
| `~/.pluginhub/saved` | 另存并还原时存下的修改，不会自动删 |
| `~/Library/LaunchAgents/com.yuwan.pluginhub.plist` | macOS 上后台任务的 launchd 描述文件，关掉后台检查和自动更新就会删掉 |

## 还没做的

- 没有 Linux 版。
- 技能还不能从 git 仓库添加，现在管的是本机已有的技能。
- 安装包没有代码签名，macOS 版也没有公证。
- 只管 Claude Code 和 Codex 这两个 app。

## 开发

界面是 React，程序是 Rust（Tauri 2）。怎么构建、打包、测试，见 [pluginhub/README.md](pluginhub/README.md)。

## 许可

[MIT](LICENSE)
