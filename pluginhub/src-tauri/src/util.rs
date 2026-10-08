//! 基础工具：路径常量、日志、JSON 文件、目录链接、子进程、跨进程锁。
//! 和操作系统打交道的部分在 platform/ 里（清单见 platform/mod.rs），这里只用它导出的名字，不写 `#[cfg]`。
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, SecondsFormat};
use serde_json::{Map, Value};

use crate::platform;

pub const HUB_VERSION: &str = "1.4.0";
/// 路径分隔符：规范化后的路径都用它
pub const SEP: char = std::path::MAIN_SEPARATOR;

pub fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub static HOME: LazyLock<PathBuf> = LazyLock::new(|| env_path(platform::HOME_VAR).unwrap_or_else(|| PathBuf::from(".")));
pub static HUB_DIR: LazyLock<PathBuf> = LazyLock::new(|| env_path("PLUGINHUB_HOME").unwrap_or_else(|| HOME.join(".pluginhub")));
pub static CONFIG_PATH: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("config.json"));
pub static STATE_PATH: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("state.json"));
pub static LOG_PATH: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("hub.log"));
pub static LOCK_PATH: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("hub.lock"));
pub static SERVER_INFO: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("server.json"));
pub static USAGE_PATH: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("usage.json"));
/// 受管插件的唯一存放处：每个插件一份 git 克隆，两边的 app 都链接到这里
pub static PLUGINS_DIR: LazyLock<PathBuf> = LazyLock::new(|| env_path("YUWAN_PLUGINS").unwrap_or_else(|| HOME.join(".yuwanplugins")));
/// 旧版本把克隆放在这里，用到时会挪到 PLUGINS_DIR
pub static LEGACY_REPOS_DIR: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("repos"));
pub static BACKUP_DIR: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("backups"));
/// 「另存并还原」时，插件文件夹里的修改整份存在这里
pub static SAVED_DIR: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("saved"));
pub static TMP_DIR: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("tmp"));
pub static DAEMON_LOCK: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("daemon.lock"));
pub static DAEMON_INFO: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("daemon.json"));

pub static CLAUDE_HOME: LazyLock<PathBuf> = LazyLock::new(|| HOME.join(".claude"));
pub static CLAUDE_PLUGINS: LazyLock<PathBuf> = LazyLock::new(|| CLAUDE_HOME.join("plugins"));
/// 这里带 .claude-plugin/plugin.json 的文件夹会被就地加载成 <名字>@skills-dir
pub static CLAUDE_SKILLS: LazyLock<PathBuf> = LazyLock::new(|| CLAUDE_HOME.join("skills"));
pub static CODEX_HOME: LazyLock<PathBuf> = LazyLock::new(|| env_path("CODEX_HOME").unwrap_or_else(|| HOME.join(".codex")));
pub static PERSONAL_MARKETPLACE: LazyLock<PathBuf> = LazyLock::new(|| HOME.join(".agents").join("plugins").join("marketplace.json"));
pub const CODEX_MANIFEST: &str = ".codex-plugin/plugin.json";
/// 从一个 app 移植到另一个 app 的插件副本里放这个文件，记着从哪来
pub const PORT_MARK: &str = ".pluginhub-port.json";
/// 移植时不复制：两边各自的清单、钩子、MCP 和 App 集成（格式不通用），以及 git 和缓存
pub const PORT_SKIP: &[&str] = &[".git", ".codex-plugin", ".claude-plugin", "hooks", ".mcp.json", ".app.json", PORT_MARK, "__pycache__"];

pub const DEFAULT_PORT: u16 = 8765;
pub const APPS: [&str; 2] = ["claude", "codex"];
pub const IDLE_EXIT_MINUTES: u64 = 90;
pub const KEEP_BACKUPS: usize = 3;
/// 后台检查每天一次
pub const GUARD_MINUTES: i64 = 1440;

pub fn app_name(app: &str) -> &'static str {
    match app {
        "claude" => "Claude Code",
        "codex" => "Codex",
        _ => "?",
    }
}

/// 给用户看的错误，消息原样显示在页面和日志里。LocalChanges：插件文件夹里有修改，这次不拉取。
#[derive(Debug, Clone)]
pub enum HubError {
    Msg(String),
    LocalChanges(String),
}

impl std::fmt::Display for HubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HubError::Msg(s) | HubError::LocalChanges(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for HubError {}

impl From<io::Error> for HubError {
    fn from(e: io::Error) -> Self {
        HubError::Msg(e.to_string())
    }
}

impl From<serde_json::Error> for HubError {
    fn from(e: serde_json::Error) -> Self {
        HubError::Msg(format!("JSON 格式不对：{e}"))
    }
}

impl From<String> for HubError {
    fn from(s: String) -> Self {
        HubError::Msg(s)
    }
}

pub type R<T> = Result<T, HubError>;

#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => { return Err($crate::util::HubError::Msg(format!($($arg)*))) };
}

// ---------------------------------------------------------------- 时间和日志

pub fn now() -> DateTime<Local> {
    Local::now()
}

pub fn stamp() -> String {
    now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

pub fn log(message: &str) {
    let _ = fs::create_dir_all(&*HUB_DIR);
    if let Ok(mut f) = fs::OpenOptions::new().append(true).create(true).open(&*LOG_PATH) {
        let _ = writeln!(f, "{}  {}", now().format("%Y-%m-%d %H:%M:%S"), message);
    }
}

pub fn tail_log(n: usize) -> Vec<String> {
    let Ok(bytes) = fs::read(&*LOG_PATH) else { return Vec::new() };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > 3000 {
        // 日志只留最近一段，不无限增长
        let keep = &lines[lines.len() - 1500..];
        let _ = fs::write(&*LOG_PATH, keep.join("\n") + "\n");
    }
    lines.iter().rev().take(n).map(|s| s.to_string()).collect()
}

// ---------------------------------------------------------------- JSON 文件

pub fn read_json(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    serde_json::from_str(text).ok()
}

/// 读 JSON 对象；文件不存在、不是 JSON 或不是对象都返回空对象
pub fn read_obj(path: &Path) -> Map<String, Value> {
    match read_json(path) {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

pub fn write_json(path: &Path, data: &Value) -> R<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.tmp"));
    fs::write(&tmp, serde_json::to_string_pretty(data)? + "\n")?;
    fs::rename(&tmp, path)?;
    Ok(())
}

// 取 JSON 对象里的字段，缺了就给默认值
pub fn gs<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn gb(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

static EMPTY: Vec<Value> = Vec::new();

pub fn ga<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&EMPTY)
}

pub fn go<'a>(v: &'a Value, key: &str) -> Option<&'a Map<String, Value>> {
    v.get(key).and_then(Value::as_object)
}

/// map[key] 不是对象就换成空对象，返回它
pub fn sub<'a>(m: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    if !m.get(key).map(Value::is_object).unwrap_or(false) {
        m.insert(key.to_string(), Value::Object(Map::new()));
    }
    m.get_mut(key).and_then(Value::as_object_mut).expect("just inserted")
}

// ---------------------------------------------------------------- 路径

/// 规范化后的路径文本，用来比较两个路径是不是同一个：绝对路径、去掉结尾的分隔符（Windows 上还统一成反斜杠、不分大小写）
pub fn norm(p: &Path) -> String {
    platform::norm(&std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()))
}

pub fn same_path(a: Option<&Path>, b: Option<&Path>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) if !a.as_os_str().is_empty() && !b.as_os_str().is_empty() => norm(a) == norm(b),
        _ => false,
    }
}

/// path 在 root 这个文件夹里面（root 自己不算）
pub fn under(path: &Path, root: &Path) -> bool {
    let (p, r) = (norm(path), norm(root));
    p.len() > r.len() && p.starts_with(&r) && p[r.len()..].starts_with(SEP)
}

/// 把链接都解析掉以后的真实路径。app 报告它读的文件时，可能写的是链接的路径，也可能是指向的真实路径，
/// 比较时两边都解析了再比
pub fn real(p: &Path) -> PathBuf {
    fs::canonicalize(p).map(platform::plain).unwrap_or_else(|_| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()))
}

pub fn same_real(a: &Path, b: &Path) -> bool {
    same_path(Some(&real(a)), Some(&real(b)))
}

pub fn under_real(path: &Path, root: &Path) -> bool {
    under(&real(path), &real(root))
}

pub fn exe_path() -> Option<PathBuf> {
    std::env::current_exe().ok().map(platform::plain)
}

pub fn mtime(p: &Path) -> f64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn mtime_of(t: SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn local_time(secs: f64) -> String {
    let t = UNIX_EPOCH + Duration::from_secs_f64(secs.max(0.0));
    DateTime::<Local>::from(t).format("%m-%d %H:%M").to_string()
}

/// 配置文件最后一次被改的时间，用来判断是不是别的程序刚动过它
pub fn changed_at(path: &Path) -> String {
    let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let t = mtime(path);
    if t > 0.0 {
        format!("{name} 修改于 {}", local_time(t))
    } else {
        format!("{name} 不存在")
    }
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

/// 给人看的路径写法
pub fn display(p: &Path) -> String {
    platform::plain(p.to_path_buf()).to_string_lossy().to_string()
}

// ---------------------------------------------------------------- 目录链接：建、查、删三个基本操作由平台提供（Windows 用 junction，macOS 用符号链接），
// 这里是用它们拼出来的操作。删链接只删链接本身，不碰它指向的文件夹。

pub fn links_to(link: &Path, target: &Path) -> bool {
    platform::is_link(link) && platform::link_target(link).map(|t| same_path(Some(&t), Some(target))).unwrap_or(false)
}

/// 让 link 指向 target。已经指向它就不动；是别的链接就换掉；是真实文件夹就报错不碰。返回是否改动。
pub fn make_link(link: &Path, target: &Path) -> R<bool> {
    if links_to(link, target) {
        return Ok(false);
    }
    if platform::is_link(link) {
        platform::remove_link(link)?;
    } else if fs::symlink_metadata(link).is_ok() {
        bail!("{} 已经存在，而且不是链接，没有动它。", display(link));
    }
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    platform::create_link(link, target).map_err(|e| HubError::Msg(format!("建链接 {} 失败：{e}", display(link))))?;
    Ok(true)
}

/// 删文件夹。是链接就只删链接本身，不碰指向的文件夹；.git 里的只读文件也能删。
pub fn rmtree(path: &Path) -> R<()> {
    if platform::is_link(path) {
        platform::remove_link(path)?;
        return Ok(());
    }
    if !path.exists() {
        return Ok(());
    }
    if let Err(first) = fs::remove_dir_all(path) {
        for entry in walkdir::WalkDir::new(path).into_iter().flatten() {
            if let Ok(meta) = entry.metadata() {
                let mut perm = meta.permissions();
                if perm.readonly() {
                    #[allow(clippy::permissions_set_readonly_false)]
                    perm.set_readonly(false);
                    let _ = fs::set_permissions(entry.path(), perm);
                }
            }
        }
        fs::remove_dir_all(path).map_err(|e| HubError::Msg(format!("删除 {} 失败：{e}（{first}）", display(path))))?;
    }
    Ok(())
}

pub fn remove_path(p: &Path) -> R<()> {
    if platform::is_link(p) || p.is_dir() {
        rmtree(p)
    } else if fs::symlink_metadata(p).is_ok() {
        fs::remove_file(p)?;
        Ok(())
    } else {
        Ok(())
    }
}

/// 把文件夹挪到别处：同一个盘直接改名，不行就复制再删
pub fn move_path(src: &Path, dst: &Path) -> R<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    if src.is_dir() {
        copy_dir(src, dst, &[])?;
        rmtree(src)
    } else {
        fs::copy(src, dst)?;
        fs::remove_file(src)?;
        Ok(())
    }
}

/// 递归复制文件夹（已有的文件覆盖）。里面的目录链接照样建成链接，skip 里列的名字不复制。
pub fn copy_dir(src: &Path, dst: &Path, skip: &[&str]) -> R<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if skip.iter().any(|s| OsStr::new(s) == name.as_os_str()) {
            continue;
        }
        let (from, to) = (entry.path(), dst.join(&name));
        if platform::is_link(&from) {
            if let Some(target) = platform::link_target(&from) {
                let _ = remove_path(&to);
                make_link(&to, &target)?;
            }
        } else if from.is_dir() {
            copy_dir(&from, &to, skip)?;
        } else {
            let _ = remove_path(&to);
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 版本号比较用的键：数字段按数值比，其余按文字比
pub type VersionKey = Vec<(u8, u64, String)>;

pub fn version_key(text: &str) -> VersionKey {
    text.split(['.', '-', '+', '_'])
        .map(|x| match x.parse::<u64>() {
            Ok(n) if !x.is_empty() => (0, n, String::new()),
            _ => (1, 0, x.to_string()),
        })
        .collect()
}

/// 版本号最大的那个（找命令行时，同一个程序装了几个版本取最新）
pub fn newest(found: Vec<(VersionKey, PathBuf)>) -> Option<PathBuf> {
    found.into_iter().max_by(|a, b| a.0.cmp(&b.0)).map(|x| x.1)
}

// ---------------------------------------------------------------- 子进程

pub struct Out {
    pub code: i32,
    pub out: String,
    pub err: String,
}

fn stem(program: &str) -> String {
    Path::new(program).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| program.to_string())
}

/// 子进程环境：定时任务里没有桌面 App 的代理变量，用配置里记下的补上
fn apply_proxy(cmd: &mut Command) {
    if let Some(proxy) = crate::store::load_config().proxy {
        for (k, v) in proxy {
            if std::env::var_os(&k).is_none() {
                cmd.env(k, v);
            }
        }
    }
}

/// 等一个已经配好的命令结束（有超时）。label 只用在错误信息里；console 为真表示是系统自带的命令，输出按系统控制台的编码解码。
pub(crate) fn exec(mut cmd: Command, label: &str, timeout: u64, check: bool, console: bool) -> R<Out> {
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => bail!("找不到程序：{label}"),
        Err(e) => bail!("启动 {} 失败：{e}", stem(label)),
    };
    let mut out_pipe = child.stdout.take().expect("piped");
    let mut err_pipe = child.stderr.take().expect("piped");
    let t_out = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out_pipe.read_to_end(&mut v);
        v
    });
    let t_err = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err_pipe.read_to_end(&mut v);
        v
    });
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("{} 超时（{timeout} 秒）", label);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => bail!("等待 {} 结束时出错：{e}", stem(label)),
        }
    };
    let decode = |v: Vec<u8>| if console { platform::decode_console(&v) } else { String::from_utf8_lossy(&v).into_owned() };
    let out = decode(t_out.join().unwrap_or_default());
    let err = decode(t_err.join().unwrap_or_default());
    let code = status.code().unwrap_or(-1);
    if check && code != 0 {
        let detail = if err.trim().is_empty() { out.trim() } else { err.trim() };
        let detail: String = detail.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
        bail!("{} 失败：{detail}", label);
    }
    Ok(Out { code, out, err })
}

/// 运行一个程序并等它结束（不弹窗、有超时）。check 为真时退出码不是 0 就报错；console 见 exec。
pub fn run(program: &str, args: &[&str], cwd: Option<&Path>, timeout: u64, check: bool, console: bool) -> R<Out> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    platform::prepare(&mut cmd);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    apply_proxy(&mut cmd);
    let shown: Vec<&str> = args.iter().take(2).copied().collect();
    let label = if program.contains(SEP) { format!("{} {}", stem(program), shown.join(" ")) } else { format!("{program} {}", shown.join(" ")) };
    exec(cmd, label.trim(), timeout, check, console)
}

// ---------------------------------------------------------------- 后台任务的命令

/// 后台任务、开机自启用的命令：(程序, 参数, 工作目录)。后台跑的永远是这个程序自己：
/// `--run auto` 由系统任务每次调用，`--run daemon` 是常驻的后台进程。
pub fn launcher(args: &[&str]) -> (String, Vec<String>, String) {
    let exe = exe_path().unwrap_or_else(|| PathBuf::from(platform::EXE_NAME));
    let workdir = exe.parent().map(display).unwrap_or_else(|| ".".into());
    let mut full = vec!["--run".to_string()];
    full.extend(args.iter().map(|s| s.to_string()));
    (display(&exe), full, workdir)
}

/// 写进计划任务的 XML 和 launchd 的 plist 之前转义
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// 后台任务的命令拼成一行，页面上显示用
pub fn launcher_line(args: &[&str]) -> String {
    let (program, a, _) = launcher(args);
    let mut all = vec![program];
    all.extend(a);
    platform::cmdline(&all)
}

pub fn find_git() -> R<PathBuf> {
    platform::which_git().ok_or_else(|| HubError::Msg(format!("找不到 git，{}", platform::GIT_HINT)))
}

// ---------------------------------------------------------------- 跨进程锁

/// 页面和后台任务是两个进程，用锁文件保证同一时间只有一个在改东西。丢掉这个值就是解锁。
pub struct HubLock;

impl Drop for HubLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&*LOCK_PATH);
    }
}

pub fn hub_lock(wait_secs: u64) -> R<HubLock> {
    fs::create_dir_all(&*HUB_DIR)?;
    let deadline = Instant::now() + Duration::from_secs(wait_secs);
    loop {
        match fs::OpenOptions::new().write(true).create_new(true).open(&*LOCK_PATH) {
            Ok(mut f) => {
                let _ = f.write_all(std::process::id().to_string().as_bytes());
                return Ok(HubLock);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let holder: u32 = fs::read_to_string(&*LOCK_PATH).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                // 残留的锁：拿锁的进程已经不在了（比如操作到一半窗口被关掉），或者是半小时前留下的
                let age = mtime_of(SystemTime::now()) - mtime(&LOCK_PATH);
                if (holder != 0 && !platform::process_alive(holder)) || age > 1800.0 {
                    let _ = fs::remove_file(&*LOCK_PATH);
                    continue;
                }
                if Instant::now() > deadline {
                    bail!("另一个更新任务正在运行，稍后再试。");
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_needs_a_separator_after_root() {
        let root = HOME.join("a");
        assert!(under(&root.join("b/c"), &root));
        assert!(!under(&root, &root));
        assert!(!under(&HOME.join("ab"), &root)); // 只是前缀一样，不在里面
    }

    #[test]
    fn links_are_made_replaced_and_removed() {
        let dir = std::env::temp_dir().join(format!("pluginhub-link-{}", uuid::Uuid::new_v4().simple()));
        let (a, b, link) = (dir.join("a"), dir.join("b"), dir.join("link"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        assert!(make_link(&link, &a).unwrap());
        assert!(platform::is_link(&link) && links_to(&link, &a) && !links_to(&link, &b));
        assert!(!make_link(&link, &a).unwrap()); // 已经指向它，不动
        assert!(make_link(&link, &b).unwrap()); // 换掉
        assert!(links_to(&link, &b));
        assert!(make_link(&a, &b).is_err()); // 真实文件夹不碰
        rmtree(&link).unwrap();
        assert!(!platform::is_link(&link) && b.is_dir()); // 只删链接，指向的文件夹还在
        fs::remove_dir_all(&dir).unwrap();
    }
}
