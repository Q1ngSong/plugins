//! 平台层：所有和操作系统打交道的代码都在这个文件夹里，别的模块只认这里导出的名字，自己不写 `#[cfg]`。
//!
//! 每个系统一个文件，按编译目标选一个。两个文件提供同一套东西（清单见下），换平台就是换文件；
//! 要支持新系统，照着 macos.rs 写一个新文件，再在下面加两行。
//!
//! 常量
//! - `HOME_VAR`：用户主目录的环境变量名
//! - `EXE_NAME`、`CLAUDE_EXE`、`CODEX_EXE`：程序和两边命令行的文件名，提示信息里用
//! - `GIT_HINT`：找不到 git 时提示怎么装
//!
//! 路径
//! - `plain(PathBuf) -> PathBuf`：去掉系统特有的前缀（Windows 的 `\\?\`）
//! - `norm(&Path) -> String`：绝对路径规范成可以直接比较的文本
//!
//! 目录链接（Windows 用 junction，macOS 用符号链接）
//! - `is_link`、`link_target`、`remove_link`、`create_link`
//!
//! 子进程
//! - `prepare(&mut Command)`：启动前的设置，Windows 不弹黑框，macOS 补上登录 shell 的 PATH
//! - `spawn_detached(program, args, cwd)`：后台拉起一个进程，不等它
//! - `decode_console(&[u8]) -> String`：系统自带命令的输出解码（Windows 是 OEM 代码页）
//! - `cmdline(&[String]) -> String`：参数拼成一行命令
//! - `process_alive(pid) -> bool`
//! - `attach_console()`：命令行模式接上终端（只有 Windows 的 GUI 程序需要）
//!
//! 系统
//! - `shell_open(&str)`：在资源管理器（访达）或浏览器里打开
//! - `find_claude()`、`find_codex()`、`which_git()`：找两边的命令行和 git
//!
//! 后台任务（每隔几分钟运行一次 `--run auto`）
//! - `install_schedule(minutes) -> R<(mode, 说明)>`：mode 是 "task"（系统任务）或 "startup"（开机自启拉起常驻的后台进程）
//! - `remove_schedule()`
//! - `task_status() -> Map`：installed、next_run（报不出来就空）、program（任务里写的程序路径，报不出来就空）
//! - `startup_installed() -> bool`：退路的开机自启装没装；没有退路的系统总是 false

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("还没有这个系统的实现：照 platform/macos.rs 写一个，在 platform/mod.rs 里加上");
