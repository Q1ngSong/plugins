//! macOS：目录链接用符号链接；子进程补上登录 shell 的 PATH（从 Finder 或 launchd 启动的程序 PATH 只有系统目录，
//! Homebrew、npm 装的命令都不在里面）；后台任务用 launchd（~/Library/LaunchAgents 里的 plist，开机登录后和每隔一段时间运行）。
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use serde_json::{Map, Value};

use crate::bail;
use crate::util::*;

pub const HOME_VAR: &str = "HOME";
pub const EXE_NAME: &str = "pluginhub";
pub const CLAUDE_EXE: &str = "claude";
pub const CODEX_EXE: &str = "codex";
pub const GIT_HINT: &str = "请先装 Xcode 命令行工具（终端里运行 xcode-select --install），或者用 Homebrew 装 git。";
/// 不放托盘图标：点关闭照系统的习惯来
pub const TRAY: bool = false;

/// launchd 任务的标签，plist 放在 ~/Library/LaunchAgents/<标签>.plist
const LAUNCHD_LABEL: &str = "com.yuwan.pluginhub";

// ---------------------------------------------------------------- 路径

/// macOS 的路径没有特殊前缀，原样返回
pub fn plain(p: PathBuf) -> PathBuf {
    p
}

/// 去掉结尾的斜杠（根目录除外）。文件系统默认不分大小写但保留大小写，这里不动大小写，和 Finder 显示的一致
pub fn norm(abs: &Path) -> String {
    let s = abs.to_string_lossy();
    let t = s.trim_end_matches('/');
    (if t.is_empty() { "/" } else { t }).to_string()
}

// ---------------------------------------------------------------- 目录链接（符号链接）

pub fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

/// 别的程序建的链接可能写的是相对路径，按链接所在的文件夹算
pub fn link_target(link: &Path) -> Option<PathBuf> {
    let t = fs::read_link(link).ok()?;
    if t.is_absolute() {
        Some(t)
    } else {
        Some(link.parent().map(|d| d.join(&t)).unwrap_or(t))
    }
}

/// 符号链接本身是个文件，删它用 remove_file：只删链接本身，不碰指向的文件夹
pub fn remove_link(link: &Path) -> io::Result<()> {
    fs::remove_file(link)
}

/// 写绝对路径：两边的 app 读到的就是完整路径
pub fn create_link(link: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(std::path::absolute(target)?, link)
}

// ---------------------------------------------------------------- 子进程

/// 找命令行和启动子进程时用的 PATH：登录 shell 的 PATH（Homebrew、nvm 这些都是在 shell 配置里加进去的），
/// 加上当前的 PATH、官方安装脚本用的 ~/.local/bin、Homebrew 和系统目录。只算一次。
static SEARCH_PATH: LazyLock<OsString> = LazyLock::new(|| {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |d: PathBuf| {
        if d.is_dir() && !dirs.contains(&d) {
            dirs.push(d);
        }
    };
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    let mut probe = Command::new(&shell);
    // 交互式登录 shell：.zprofile 和 .zshrc 都会读到。输出前后加标记，免得 shell 配置里打印的东西混进来
    probe.args(["-ilc", "printf '\\n<PATH>%s</PATH>' \"$PATH\""]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Ok(o) = exec(probe, "shell", 10, false, false) {
        if let Some((_, rest)) = o.out.split_once("<PATH>") {
            if let Some((path, _)) = rest.split_once("</PATH>") {
                for d in std::env::split_paths(path) {
                    push(d);
                }
            }
        }
    }
    if let Some(p) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&p) {
            push(d);
        }
    }
    push(HOME.join(".local/bin"));
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        push(PathBuf::from(d));
    }
    // nvm：每个 node 版本一个 bin 文件夹，新的在前
    if let Ok(rd) = fs::read_dir(HOME.join(".nvm/versions/node")) {
        let mut versions: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        versions.sort_by_key(|p| std::cmp::Reverse(version_key(&file_name(p))));
        for v in versions {
            push(v.join("bin"));
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
});

/// 补上完整的 PATH：npm 装的 claude、codex 是 node 脚本，要能找到 node
pub fn prepare(cmd: &mut Command) {
    cmd.env("PATH", &*SEARCH_PATH);
}

/// 系统命令的输出就是 UTF-8
pub fn decode_console(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// 在后台拉起一个进程，不等它：放进新的进程组，关掉终端也不会把它带走
pub fn spawn_detached(program: &Path, args: &[&str], cwd: Option<&Path>) -> R<()> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    prepare(&mut cmd);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.spawn().map_err(|e| HubError::Msg(format!("启动 {} 失败：{e}", display(program))))?;
    Ok(())
}

/// 把参数拼成一行命令，按 shell 的单引号规则（页面上显示用）
pub fn cmdline(args: &[String]) -> String {
    let plain = |c: char| c.is_alphanumeric() || "-_./=:@+%,".contains(c);
    args.iter()
        .map(|a| if !a.is_empty() && a.chars().all(plain) { a.clone() } else { format!("'{}'", a.replace('\'', "'\\''")) })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // 信号 0 不真的发信号，只看进程在不在；别人的进程会报没权限，那也说明它在
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// 程序本来就连着终端，不用做什么
pub fn attach_console() {}

// ---------------------------------------------------------------- 系统

/// 在访达或浏览器里打开一个文件夹或网址
pub fn shell_open(target: &str) -> R<()> {
    let r = run("/usr/bin/open", &[target], None, 30, false, false)?;
    if r.code != 0 {
        bail!("打不开 {target}：{}", r.err.trim());
    }
    Ok(())
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&*SEARCH_PATH).map(|d| d.join(name)).find(|p| p.is_file())
}

/// 只认有执行权限的文件（跟着链接看）
fn real_exe(p: &Path) -> Option<PathBuf> {
    match fs::metadata(p) {
        Ok(m) if m.is_file() && m.permissions().mode() & 0o111 != 0 => Some(p.to_path_buf()),
        _ => None,
    }
}

/// 先找 PATH 上的 claude（SEARCH_PATH 里已经有 ~/.local/bin 和 Homebrew）和 ~/.claude/local 里的；没有再看桌面 App 自带的
/// （~/Library/Application Support/Claude/claude-code/<版本>/claude.app/…，新版本在 <版本> 下面多一层哈希文件夹），取最新。
/// 下载到一半的版本只有一个 .partial 文件，自然跳过
pub fn find_claude() -> Option<PathBuf> {
    if let Some(p) = [on_path("claude"), Some(HOME.join(".claude/local/claude"))].into_iter().flatten().find_map(|p| real_exe(&p)) {
        return Some(p);
    }
    let mut found = Vec::new();
    if let Ok(rd) = fs::read_dir(HOME.join("Library/Application Support/Claude/claude-code")) {
        for d in rd.flatten() {
            let v = d.path();
            // 直接在版本文件夹里，或者再往下一层
            let mut dirs = vec![v.clone()];
            if let Ok(sub) = fs::read_dir(&v) {
                dirs.extend(sub.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
            }
            if let Some(exe) = dirs.iter().find_map(|p| real_exe(&p.join("claude.app/Contents/MacOS/claude")).or_else(|| real_exe(&p.join("claude")))) {
                found.push((version_key(&d.file_name().to_string_lossy()), exe));
            }
        }
    }
    newest(found)
}

/// 先看 Codex 自己下载的独立版本（~/.codex/packages/standalone/releases/<版本>/bin），取最新；
/// 再看 Codex 桌面版和 ChatGPT 桌面版自带的命令行（Contents/Resources/codex-cli）；
/// npm 装的 codex 是个 node 脚本，真正的程序在它旁边的 vendor 文件夹里，直接用那个，不依赖 PATH 里有没有 node
pub fn find_codex() -> Option<PathBuf> {
    let mut found = Vec::new();
    if let Ok(rd) = fs::read_dir(CODEX_HOME.join("packages/standalone/releases")) {
        for d in rd.flatten() {
            if let Some(exe) = real_exe(&d.path().join("bin/codex")) {
                found.push((version_key(&d.file_name().to_string_lossy()), exe));
            }
        }
    }
    if let Some(p) = newest(found) {
        return Some(p);
    }
    let mut apps: Vec<PathBuf> = Vec::new();
    for base in [PathBuf::from("/Applications"), HOME.join("Applications")] {
        for name in ["Codex.app", "ChatGPT.app"] {
            apps.push(base.join(name).join("Contents/Resources/codex-cli"));
        }
    }
    apps.push(CODEX_HOME.join("plugins/.plugin-appserver/codex-cli"));
    for cli in apps {
        if let Some(exe) = real_exe(&cli.join("CodexCLI.app/Contents/MacOS/codex")).or_else(|| real_exe(&cli.join("bin/codex"))) {
            return Some(exe);
        }
    }
    if let Some(p) = on_path("codex") {
        let target = fs::canonicalize(&p).unwrap_or(p);
        if target.extension().map(|e| e == "js").unwrap_or(false) {
            // npm 包：bin/codex.js 旁边的 vendor/<平台>/codex/codex，或者平台包里的同一位置
            let pkg = target.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
            let triple = format!("{}-apple-darwin", std::env::consts::ARCH);
            let platform = format!("codex-darwin-{}", if std::env::consts::ARCH == "aarch64" { "arm64" } else { "x64" });
            let rel = format!("vendor/{triple}/codex/codex");
            for candidate in [pkg.join(&rel), pkg.join("node_modules/@openai").join(&platform).join(&rel)] {
                if let Some(exe) = real_exe(&candidate) {
                    return Some(exe);
                }
            }
        } else if let Some(exe) = real_exe(&target) {
            return Some(exe);
        }
    }
    real_exe(&CODEX_HOME.join(".sandbox-bin/codex"))
}

pub fn which_git() -> Option<PathBuf> {
    on_path("git")
}

// ---------------------------------------------------------------- 后台任务：launchd

const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
{args}
  </array>
  <key>WorkingDirectory</key>
  <string>{workdir}</string>
  <key>StartInterval</key>
  <integer>{seconds}</integer>
  <key>RunAtLoad</key>
  <true/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#;

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn plist_path() -> PathBuf {
    HOME.join("Library/LaunchAgents").join(format!("{LAUNCHD_LABEL}.plist"))
}

/// 当前用户的 launchd 域
fn domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

fn launchctl(args: &[&str]) -> R<Out> {
    run("/bin/launchctl", args, None, 30, false, false)
}

fn target() -> String {
    format!("{}/{LAUNCHD_LABEL}", domain())
}

/// 任务登记了没有
fn loaded() -> bool {
    launchctl(&["print", &target()]).map(|o| o.code == 0).unwrap_or(false)
}

/// 任务里写的程序路径（plist 是我们自己写的，格式固定）
fn program_in(plist: &Path) -> String {
    let text = fs::read_to_string(plist).unwrap_or_default();
    let re = regex::Regex::new(r"<key>ProgramArguments</key>\s*<array>\s*<string>([^<]*)</string>").expect("regex");
    re.captures(&text).map(|c| xml_unescape(&c[1])).unwrap_or_default()
}

/// 写 plist 并登记到 launchd：登录后（RunAtLoad）和每隔 minutes 分钟（StartInterval）运行一次 `--run auto`，
/// 它的输出写到 ~/.pluginhub/launchd.log。已经登记过的先注销，bootstrap 不接受重复。
/// 登记和登录后 launchd 都会马上拉起它，所以传 --delay 让它等两分钟再开始（和 Windows 的计划任务一样），
/// 免得和用户正在做的操作抢锁；重新登记时 bootout 会把还在等的那个结束掉，重新计时
pub fn install_schedule(minutes: i64) -> R<(&'static str, String)> {
    let (program, args, workdir) = launcher(&["auto", "--delay", "120"]);
    let strings: Vec<String> = std::iter::once(&program).chain(&args).map(|s| format!("    <string>{}</string>", xml_escape(s))).collect();
    let xml = PLIST
        .replace("{label}", LAUNCHD_LABEL)
        .replace("{args}", &strings.join("\n"))
        .replace("{workdir}", &xml_escape(&workdir))
        .replace("{seconds}", &(minutes * 60).to_string())
        .replace("{log}", &xml_escape(&display(&HUB_DIR.join("launchd.log"))));
    let plist = plist_path();
    if let Some(parent) = plist.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir_all(&*HUB_DIR)?;
    fs::write(&plist, xml)?;
    if loaded() {
        // 注销是异步的，紧接着 bootstrap 会报错，等它真的卸掉
        let _ = launchctl(&["bootout", &target()]);
        for _ in 0..20 {
            if !loaded() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    let r = launchctl(&["bootstrap", &domain(), &display(&plist)])?;
    if r.code != 0 && !loaded() {
        // 老一点的系统不认 bootstrap，退回老命令
        let legacy = launchctl(&["load", "-w", &display(&plist)])?;
        if legacy.code != 0 && !loaded() {
            let detail = if r.err.trim().is_empty() { r.out.trim() } else { r.err.trim() };
            bail!("登记 launchd 任务失败：{detail}");
        }
    }
    Ok(("task", format!("后台任务：开机登录后和每隔一段时间运行一次（每 {minutes} 分钟，launchd）")))
}

pub fn remove_schedule() {
    let _ = launchctl(&["bootout", &target()]);
    let _ = fs::remove_file(plist_path());
}

/// launchd 不报下次什么时候跑，next_run 留空，schedule.rs 按上次跑过的时间算
pub fn task_status() -> Map<String, Value> {
    let mut m = Map::new();
    let plist = plist_path();
    let on = plist.is_file() && loaded();
    m.insert("installed".into(), Value::Bool(on));
    if on {
        m.insert("program".into(), Value::from(program_in(&plist)));
    }
    m
}

/// launchd 任务本身就是开机登录后运行，没有退路
pub fn startup_installed() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmdline_quotes_what_the_shell_would_eat() {
        let args = ["/Applications/插件中心.app/Contents/MacOS/pluginhub", "--run", "auto", "it's here"].map(String::from);
        assert_eq!(cmdline(&args), "/Applications/插件中心.app/Contents/MacOS/pluginhub --run auto 'it'\\''s here'");
    }

    #[test]
    fn program_is_read_back_from_plist() {
        let dir = std::env::temp_dir().join(format!("pluginhub-plist-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.plist");
        let xml = PLIST.replace("{args}", "    <string>/a &amp; b/pluginhub</string>\n    <string>--run</string>").replace("{label}", "t");
        fs::write(&p, xml).unwrap();
        assert_eq!(program_in(&p), "/a & b/pluginhub");
        fs::remove_dir_all(&dir).unwrap();
    }
}
