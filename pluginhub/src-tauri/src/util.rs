//! 基础工具：路径常量、日志、JSON 文件、目录链接（junction）、子进程、找命令行、跨进程锁。
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::FileTypeExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, SecondsFormat};
use serde_json::{Map, Value};

pub const HUB_VERSION: &str = "1.2.0";

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub static HOME: LazyLock<PathBuf> = LazyLock::new(|| env_path("USERPROFILE").unwrap_or_else(|| PathBuf::from(".")));
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
/// 放在任务计划根目录：新建任务文件夹需要管理员权限
pub const TASK_NAME: &str = "PluginHub-AutoUpdate";
pub const STARTUP_NAME: &str = "插件中心自动更新";
pub const IDLE_EXIT_MINUTES: u64 = 90;
pub const KEEP_BACKUPS: usize = 3;
/// 后台检查每天一次
pub const GUARD_MINUTES: i64 = 1440;
pub const NO_WINDOW: u32 = 0x0800_0000; // CREATE_NO_WINDOW：不弹黑框
pub const DETACHED: u32 = 0x0000_0008 | 0x0000_0200; // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP

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

/// 规范化后的路径文本（绝对路径、统一分隔符、不分大小写），用来比较两个路径是不是同一个
pub fn norm(p: &Path) -> String {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
    let t = s.trim_end_matches('\\');
    (if t.len() < 2 { s.as_str() } else { t }).to_lowercase()
}

pub fn same_path(a: Option<&Path>, b: Option<&Path>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) if !a.as_os_str().is_empty() && !b.as_os_str().is_empty() => norm(a) == norm(b),
        _ => false,
    }
}

/// 去掉 \\?\ 前缀：路径要写进定时任务、快捷方式和页面，用普通写法
pub fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => p,
    }
}

pub fn exe_path() -> Option<PathBuf> {
    std::env::current_exe().ok().map(plain)
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

pub fn display(p: &Path) -> String {
    plain(p.to_path_buf()).to_string_lossy().to_string()
}

// ---------------------------------------------------------------- 目录链接（junction）：普通用户就能建，删的时候只删链接本身

pub fn is_junction(p: &Path) -> bool {
    fs::symlink_metadata(p).map(|m| m.file_type().is_symlink_dir()).unwrap_or(false)
}

pub fn link_target(link: &Path) -> Option<PathBuf> {
    let t = fs::read_link(link).ok()?;
    let s = t.to_string_lossy().to_string();
    let s = s.strip_prefix(r"\\?\").or_else(|| s.strip_prefix(r"\??\")).unwrap_or(&s);
    Some(PathBuf::from(s))
}

pub fn links_to(link: &Path, target: &Path) -> bool {
    is_junction(link) && link_target(link).map(|t| same_path(Some(&t), Some(target))).unwrap_or(false)
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain([0]).collect()
}

/// 用 FSCTL_SET_REPARSE_POINT 建目录链接（和 Python 的 _winapi.CreateJunction 一样的做法）
fn create_junction(link: &Path, target: &Path) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    const FSCTL_SET_REPARSE_POINT: u32 = 0x000900A4;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;

    let abs = std::path::absolute(target)?;
    let t = display(&abs).replace('/', "\\");
    let subst: Vec<u16> = format!(r"\??\{t}").encode_utf16().collect();
    let print: Vec<u16> = t.encode_utf16().collect();
    let subst_len = (subst.len() * 2) as u16;
    let print_len = (print.len() * 2) as u16;
    let path_bytes = (subst.len() + 1 + print.len() + 1) * 2;
    let data_len = (8 + path_bytes) as u16;
    let mut buf: Vec<u8> = Vec::with_capacity(8 + data_len as usize);
    buf.extend(IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buf.extend(data_len.to_le_bytes());
    buf.extend(0u16.to_le_bytes());
    buf.extend(0u16.to_le_bytes()); // SubstituteNameOffset
    buf.extend(subst_len.to_le_bytes());
    buf.extend((subst_len + 2).to_le_bytes()); // PrintNameOffset
    buf.extend(print_len.to_le_bytes());
    for c in subst.iter().chain([0u16].iter()).chain(print.iter()).chain([0u16].iter()) {
        buf.extend(c.to_le_bytes());
    }
    fs::create_dir(link)?;
    let name = wide(link.as_os_str());
    unsafe {
        let h = CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            let e = io::Error::last_os_error();
            let _ = fs::remove_dir(link);
            return Err(e);
        }
        let mut returned = 0u32;
        let ok = DeviceIoControl(
            h,
            FSCTL_SET_REPARSE_POINT,
            buf.as_ptr().cast(),
            buf.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        );
        let e = io::Error::last_os_error();
        CloseHandle(h);
        if ok == 0 {
            let _ = fs::remove_dir(link);
            return Err(e);
        }
    }
    Ok(())
}

/// 让 link 指向 target。已经指向它就不动；是别的链接就换掉；是真实文件夹就报错不碰。返回是否改动。
pub fn make_junction(link: &Path, target: &Path) -> R<bool> {
    if links_to(link, target) {
        return Ok(false);
    }
    if is_junction(link) {
        fs::remove_dir(link)?;
    } else if link.exists() {
        bail!("{} 已经存在，而且不是链接，没有动它。", display(link));
    }
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    create_junction(link, target).map_err(|e| HubError::Msg(format!("建链接 {} 失败：{e}", display(link))))?;
    Ok(true)
}

/// 删文件夹。是链接就只删链接本身，不碰指向的文件夹；.git 里的只读文件也能删。
pub fn rmtree(path: &Path) -> R<()> {
    if is_junction(path) {
        fs::remove_dir(path)?;
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
    if is_junction(p) || p.is_dir() {
        rmtree(p)
    } else if p.exists() {
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
        if is_junction(&from) {
            if let Some(target) = link_target(&from) {
                let _ = remove_path(&to);
                make_junction(&to, &target)?;
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
pub fn version_key(text: &str) -> Vec<(u8, u64, String)> {
    text.split(['.', '-', '+', '_'])
        .map(|x| match x.parse::<u64>() {
            Ok(n) if !x.is_empty() => (0, n, String::new()),
            _ => (1, 0, x.to_string()),
        })
        .collect()
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

/// 用 OEM 代码页解码（schtasks 这类系统命令的输出）
pub fn decode_oem(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{MultiByteToWideChar, CP_OEMCP};
    if bytes.is_empty() {
        return String::new();
    }
    unsafe {
        let n = MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut w = vec![0u16; n as usize];
        MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), bytes.len() as i32, w.as_mut_ptr(), n);
        String::from_utf16_lossy(&w)
    }
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

/// 运行一个程序并等它结束（不弹窗、有超时）。check 为真时退出码不是 0 就报错。
pub fn run(program: &str, args: &[&str], cwd: Option<&Path>, timeout: u64, check: bool, oem: bool) -> R<Out> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).creation_flags(NO_WINDOW);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    apply_proxy(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => bail!("找不到程序：{program}"),
        Err(e) => bail!("启动 {} 失败：{e}", stem(program)),
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
                    let shown: Vec<&str> = args.iter().take(2).copied().collect();
                    bail!("{} {} 超时（{timeout} 秒）", stem(program), shown.join(" "));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => bail!("等待 {} 结束时出错：{e}", stem(program)),
        }
    };
    let decode = |v: Vec<u8>| if oem { decode_oem(&v) } else { String::from_utf8_lossy(&v).into_owned() };
    let out = decode(t_out.join().unwrap_or_default());
    let err = decode(t_err.join().unwrap_or_default());
    let code = status.code().unwrap_or(-1);
    if check && code != 0 {
        let detail = if err.trim().is_empty() { out.trim() } else { err.trim() };
        let detail: String = detail.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
        let shown: Vec<&str> = args.iter().take(3).copied().collect();
        bail!("{} {} 失败：{detail}", stem(program), shown.join(" "));
    }
    Ok(Out { code, out, err })
}

/// 把当前进程的标准句柄改成不可继承。拉起常驻的后台进程时，不能把调用方给我们的管道带过去，
/// 否则调用方（脚本、页面服务）要等到后台进程退出才能收到"输出结束"。
fn no_inherit_std_handles() {
    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        unsafe {
            let h = GetStdHandle(which);
            if !h.is_null() && h != INVALID_HANDLE_VALUE {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// 在后台拉起一个进程，不等它（开机自启的后台进程、页面服务）
pub fn spawn_detached(program: &Path, args: &[&str], cwd: Option<&Path>) -> R<()> {
    const BREAKAWAY: u32 = 0x0100_0000; // CREATE_BREAKAWAY_FROM_JOB
    no_inherit_std_handles();
    let build = |flags: u32| {
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(flags);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        cmd
    };
    // 拉起它的程序可能在一个作业对象里（终端、IDE 常这样），作业结束时里面的进程会被一起结束。
    // 先试着脱离出去；作业不允许脱离时 spawn 会失败，再照常启动。
    if build(DETACHED | NO_WINDOW | BREAKAWAY).spawn().is_ok() {
        return Ok(());
    }
    build(DETACHED | NO_WINDOW).spawn().map_err(|e| HubError::Msg(format!("启动 {} 失败：{e}", display(program))))?;
    Ok(())
}

/// 和 Python 的 subprocess.list2cmdline 一样的引号规则，写进定时任务和快捷方式
pub fn cmdline(args: &[String]) -> String {
    let mut out = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let need = arg.is_empty() || arg.contains(' ') || arg.contains('\t');
        if need {
            out.push('"');
        }
        let mut bs = 0;
        for c in arg.chars() {
            if c == '\\' {
                bs += 1;
                continue;
            }
            if c == '"' {
                out.push_str(&"\\".repeat(bs * 2 + 1));
                out.push('"');
                bs = 0;
                continue;
            }
            out.push_str(&"\\".repeat(bs));
            bs = 0;
            out.push(c);
        }
        if need {
            out.push_str(&"\\".repeat(bs * 2));
            out.push('"');
        } else {
            out.push_str(&"\\".repeat(bs));
        }
    }
    out
}

pub fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};
    const SYNCHRONIZE: u32 = 0x0010_0000;
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false;
        }
        let alive = WaitForSingleObject(h, 0) == WAIT_TIMEOUT;
        CloseHandle(h);
        alive
    }
}

/// 在资源管理器或浏览器里打开一个文件夹或网址
pub fn shell_open(target: &str) -> R<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let verb = wide(OsStr::new("open"));
    let file = wide(OsStr::new(target));
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
    if (r as usize) <= 32 {
        bail!("打不开 {target}");
    }
    Ok(())
}

/// 系统的「启动」文件夹
pub fn startup_folder() -> PathBuf {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Startup, SHGetKnownFolderPath};
    let fallback = || env_path("APPDATA").unwrap_or_else(|| HOME.clone()).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
    unsafe {
        let mut p: *mut u16 = std::ptr::null_mut();
        if SHGetKnownFolderPath(&FOLDERID_Startup, 0, std::ptr::null_mut(), &mut p) != 0 || p.is_null() {
            return fallback();
        }
        let mut len = 0;
        while *p.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        CoTaskMemFree(p.cast());
        if s.is_empty() {
            fallback()
        } else {
            PathBuf::from(s)
        }
    }
}

/// 命令行模式连到父进程的控制台，print 的内容才看得到（GUI 程序默认没有控制台）
pub fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

// ---------------------------------------------------------------- 找命令行

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|v| std::env::split_paths(&v).map(|d| d.join(name)).find(|p| p.is_file()))
}

/// 只认真正的可执行文件；npm 装坏的 claude.exe 只有几百字节
fn real_exe(p: &Path) -> Option<PathBuf> {
    match fs::metadata(p) {
        Ok(m) if m.is_file() && m.len() > 1_000_000 => Some(p.to_path_buf()),
        _ => None,
    }
}

fn newest(found: Vec<(Vec<(u8, u64, String)>, PathBuf)>) -> Option<PathBuf> {
    found.into_iter().max_by(|a, b| a.0.cmp(&b.0)).map(|x| x.1)
}

pub fn find_claude() -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Some(appdata) = env_path("APPDATA") {
        roots.push(appdata.join("Claude").join("claude-code"));
    }
    if let Some(local) = env_path("LOCALAPPDATA") {
        // 桌面 App 是 MSIX 包：从包外看，它写进 AppData 的文件在 Packages\Claude_*\LocalCache 下
        if let Ok(rd) = fs::read_dir(local.join("Packages")) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("Claude_") {
                    roots.push(e.path().join("LocalCache").join("Roaming").join("Claude").join("claude-code"));
                }
            }
        }
    }
    let mut found = Vec::new();
    for root in roots {
        if let Ok(rd) = fs::read_dir(&root) {
            for d in rd.flatten() {
                if let Some(exe) = real_exe(&d.path().join("claude.exe")) {
                    found.push((version_key(&d.file_name().to_string_lossy()), exe));
                }
            }
        }
    }
    if let Some(p) = newest(found) {
        return Some(p);
    }
    [Some(HOME.join(".local").join("bin").join("claude.exe")), on_path("claude.exe")]
        .into_iter()
        .flatten()
        .find_map(|p| real_exe(&p))
}

pub fn find_codex() -> Option<PathBuf> {
    let mut found = Vec::new();
    if let Ok(rd) = fs::read_dir(CODEX_HOME.join("packages").join("standalone").join("releases")) {
        for d in rd.flatten() {
            if let Some(exe) = real_exe(&d.path().join("bin").join("codex.exe")) {
                found.push((version_key(&d.file_name().to_string_lossy()), exe));
            }
        }
    }
    if let Some(p) = newest(found) {
        return Some(p);
    }
    [on_path("codex.exe"), Some(CODEX_HOME.join(".sandbox-bin").join("codex.exe"))]
        .into_iter()
        .flatten()
        .find_map(|p| real_exe(&p))
}

pub fn which_git() -> Option<PathBuf> {
    on_path("git.exe").or_else(|| {
        [r"C:\Program Files\Git\cmd\git.exe", r"C:\Program Files\Git\bin\git.exe"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
    })
}

pub fn find_git() -> R<PathBuf> {
    which_git().ok_or_else(|| HubError::Msg("找不到 git，请先安装 Git for Windows。".into()))
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
                if (holder != 0 && !process_alive(holder)) || age > 1800.0 {
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
