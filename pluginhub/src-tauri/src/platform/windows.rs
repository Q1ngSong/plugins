//! Windows：目录链接用 junction（普通用户就能建）；子进程不弹黑框；系统命令的输出按 OEM 代码页解码；
//! 后台任务用计划任务（schtasks），没有权限建的机器上退到「启动」文件夹里的快捷方式，开机拉起常驻的后台进程。
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::FileTypeExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use chrono::Local;
use serde_json::{Map, Value};

use crate::bail;
use crate::util::*;

pub const HOME_VAR: &str = "USERPROFILE";
pub const EXE_NAME: &str = "pluginhub.exe";
pub const CLAUDE_EXE: &str = "claude.exe";
pub const CODEX_EXE: &str = "codex.exe";
pub const GIT_HINT: &str = "请先安装 Git for Windows。";
/// 有托盘图标：点窗口的关闭按钮不退出，缩到托盘
pub const TRAY: bool = true;

const NO_WINDOW: u32 = 0x0800_0000; // CREATE_NO_WINDOW：不弹黑框
const DETACHED: u32 = 0x0000_0008 | 0x0000_0200; // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
/// 计划任务放在根目录（新建任务文件夹需要管理员权限）
const TASK_NAME: &str = "PluginHub-AutoUpdate";
/// 「启动」文件夹里快捷方式的名字
const STARTUP_NAME: &str = "插件中心自动更新";

// ---------------------------------------------------------------- 路径

/// 去掉 \\?\ 前缀：路径要写进计划任务、快捷方式和页面，用普通写法
pub fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => p,
    }
}

/// 统一成反斜杠、去掉 \\?\ 和结尾的反斜杠、不分大小写
pub fn norm(abs: &Path) -> String {
    let s = abs.to_string_lossy().replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
    let t = s.trim_end_matches('\\');
    (if t.len() < 2 { s.as_str() } else { t }).to_lowercase()
}

// ---------------------------------------------------------------- 目录链接（junction）

pub fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p).map(|m| m.file_type().is_symlink_dir()).unwrap_or(false)
}

pub fn link_target(link: &Path) -> Option<PathBuf> {
    let t = fs::read_link(link).ok()?;
    let s = t.to_string_lossy().to_string();
    let s = s.strip_prefix(r"\\?\").or_else(|| s.strip_prefix(r"\??\")).unwrap_or(&s);
    Some(PathBuf::from(s))
}

/// junction 在系统看来是个目录，删它用 remove_dir：只删链接本身，不碰指向的文件夹
pub fn remove_link(link: &Path) -> io::Result<()> {
    fs::remove_dir(link)
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain([0]).collect()
}

/// 用 FSCTL_SET_REPARSE_POINT 建 junction（和 Python 的 _winapi.CreateJunction 一样的做法）
pub fn create_link(link: &Path, target: &Path) -> io::Result<()> {
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

// ---------------------------------------------------------------- 子进程

/// 不弹黑框
pub fn prepare(cmd: &mut Command) {
    cmd.creation_flags(NO_WINDOW);
}

/// 用 OEM 代码页解码（schtasks 这类系统命令的输出）
pub fn decode_console(bytes: &[u8]) -> String {
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

/// 把参数拼成一行命令，照 Python 的 subprocess.list2cmdline 的引号规则：写进计划任务和快捷方式
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

/// 命令行模式连到父进程的控制台，print 的内容才看得到（GUI 程序默认没有控制台）
pub fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

// ---------------------------------------------------------------- 系统

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
fn startup_folder() -> PathBuf {
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

/// 先看桌面 App 自带的（AppData 里按版本号分文件夹，新版本在 <版本> 下面多一层哈希文件夹），取最新；
/// 再看官方安装脚本装的 ~/.local/bin，最后找 PATH
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
                let v = d.path();
                // 直接在版本文件夹里，或者再往下一层
                let mut dirs = vec![v.clone()];
                if let Ok(sub) = fs::read_dir(&v) {
                    dirs.extend(sub.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
                }
                if let Some(exe) = dirs.iter().find_map(|p| real_exe(&p.join(CLAUDE_EXE))) {
                    found.push((version_key(&d.file_name().to_string_lossy()), exe));
                }
            }
        }
    }
    if let Some(p) = newest(found) {
        return Some(p);
    }
    [Some(HOME.join(".local").join("bin").join(CLAUDE_EXE)), on_path(CLAUDE_EXE)].into_iter().flatten().find_map(|p| real_exe(&p))
}

/// 先看 Codex 自己下载的独立版本（~/.codex/packages/standalone/releases/<版本>/bin），取最新；再找 PATH 和沙箱里的那份
pub fn find_codex() -> Option<PathBuf> {
    let mut found = Vec::new();
    if let Ok(rd) = fs::read_dir(CODEX_HOME.join("packages").join("standalone").join("releases")) {
        for d in rd.flatten() {
            if let Some(exe) = real_exe(&d.path().join("bin").join(CODEX_EXE)) {
                found.push((version_key(&d.file_name().to_string_lossy()), exe));
            }
        }
    }
    if let Some(p) = newest(found) {
        return Some(p);
    }
    [on_path(CODEX_EXE), Some(CODEX_HOME.join(".sandbox-bin").join(CODEX_EXE))].into_iter().flatten().find_map(|p| real_exe(&p))
}

pub fn which_git() -> Option<PathBuf> {
    on_path("git.exe").or_else(|| {
        [r"C:\Program Files\Git\cmd\git.exe", r"C:\Program Files\Git\bin\git.exe"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
    })
}

// ---------------------------------------------------------------- 后台任务：计划任务，建不了就退到「启动」文件夹

const TASK_XML: &str = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>插件中心：定时检查并更新 Claude Code 和 Codex 的插件</Description>
  </RegistrationInfo>
  <Triggers>
    {logon}
    <TimeTrigger>
      <Repetition>
        <Interval>PT{minutes}M</Interval>
        <StopAtDurationEnd>false</StopAtDurationEnd>
      </Repetition>
      <StartBoundary>{start}</StartBoundary>
      <Enabled>true</Enabled>
    </TimeTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT20M</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
      <WorkingDirectory>{workdir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#;

const LOGON_TRIGGER: &str = r#"<LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
      <Delay>PT1M</Delay>
    </LogonTrigger>"#;

fn task_user() -> String {
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let user = std::env::var("USERNAME").unwrap_or_default();
    if domain.is_empty() { user } else { format!("{domain}\\{user}") }
}

fn schtasks(args: &[&str]) -> R<Out> {
    run("schtasks", args, None, 60, false, true)
}

fn register_task(minutes: i64) -> R<String> {
    let user = xml_escape(&task_user());
    let (program, args, workdir) = launcher(&["auto"]);
    let start = (Local::now() + chrono::Duration::minutes(2)).format("%Y-%m-%dT%H:%M:%S").to_string();
    let xml_path = HUB_DIR.join("task.xml");
    let mut err = String::new();
    for with_logon in [true, false] {
        // 有的系统不允许普通权限建登录触发器，退一步只用定时
        let logon = if with_logon { LOGON_TRIGGER.replace("{user}", &user) } else { String::new() };
        let xml = TASK_XML
            .replace("{logon}", &logon)
            .replace("{minutes}", &minutes.to_string())
            .replace("{start}", &start)
            .replace("{user}", &user)
            .replace("{command}", &xml_escape(&program))
            .replace("{arguments}", &xml_escape(&cmdline(&args)))
            .replace("{workdir}", &xml_escape(&workdir));
        let mut bytes: Vec<u8> = vec![0xFF, 0xFE];
        for u in xml.encode_utf16() {
            bytes.extend(u.to_le_bytes());
        }
        fs::create_dir_all(&*HUB_DIR)?;
        fs::write(&xml_path, bytes)?;
        let r = schtasks(&["/Create", "/TN", TASK_NAME, "/XML", &display(&xml_path), "/F"])?;
        if r.code == 0 {
            return Ok(if with_logon { "开机登录后和每隔一段时间" } else { "每隔一段时间" }.into());
        }
        err = if r.err.trim().is_empty() { r.out.trim().to_string() } else { r.err.trim().to_string() };
    }
    bail!("创建定时任务失败：{err}")
}

fn unregister_task() {
    let _ = schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]);
}

fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// 计划任务在不在、下次什么时候跑。程序路径 schtasks 的 CSV 里不好拿，不报
pub fn task_status() -> Map<String, Value> {
    let mut m = Map::new();
    let Ok(r) = run("schtasks", &["/Query", "/TN", TASK_NAME, "/FO", "CSV", "/NH"], None, 30, false, true) else {
        m.insert("installed".into(), Value::Bool(false));
        return m;
    };
    if r.code != 0 {
        m.insert("installed".into(), Value::Bool(false));
        return m;
    }
    let row = r.out.lines().map(csv_fields).find(|f| f.iter().any(|x| !x.trim().is_empty())).unwrap_or_default();
    m.insert("installed".into(), Value::Bool(true));
    m.insert("next_run".into(), Value::from(row.get(1).cloned().unwrap_or_default()));
    m
}

fn startup_link() -> PathBuf {
    startup_folder().join(format!("{STARTUP_NAME}.lnk"))
}

/// 建快捷方式（PowerShell 调 WScript.Shell）；不行就退回到 .cmd 文件
fn make_shortcut(lnk: &Path, program: &str, args: &[String], workdir: &str, description: &str) -> R<PathBuf> {
    let q = |s: &str| s.replace('\'', "''");
    let arguments = cmdline(args);
    let script = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath = '{}'; $s.Arguments = '{}'; $s.WorkingDirectory = '{}'; $s.IconLocation = '{},0'; $s.Description = '{}'; $s.Save()",
        q(&display(lnk)),
        q(program),
        q(&arguments),
        q(workdir),
        q(program),
        q(description)
    );
    if let Some(parent) = lnk.parent() {
        fs::create_dir_all(parent)?;
    }
    let r = run("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script], None, 60, false, true);
    if matches!(&r, Ok(o) if o.code == 0) && lnk.is_file() {
        return Ok(lnk.to_path_buf());
    }
    let cmd = lnk.with_extension("cmd");
    fs::write(&cmd, format!("@start \"\" \"{program}\" {arguments}\r\n"))?;
    Ok(cmd)
}

fn remove_startup_links() {
    let lnk = startup_link();
    let _ = fs::remove_file(&lnk);
    let _ = fs::remove_file(lnk.with_extension("cmd"));
}

/// 优先用计划任务；建不了（没有权限）就用「启动」文件夹里的快捷方式开机拉起常驻的后台进程（schedule.rs 接着把它启动起来）
pub fn install_schedule(minutes: i64) -> R<(&'static str, String)> {
    match register_task(minutes) {
        Ok(when) => {
            remove_startup_links(); // 以前退路留下的快捷方式不要了
            Ok(("task", format!("后台任务：{when}运行一次（每 {minutes} 分钟，系统定时任务）")))
        }
        Err(e) => {
            log(&format!("系统定时任务建不了（{e}），改用开机自启的后台进程"));
            let (program, args, workdir) = launcher(&["daemon"]);
            make_shortcut(&startup_link(), &program, &args, &workdir, "插件中心：后台检查插件配置、自动更新")?;
            Ok(("startup", format!("后台任务：开机后自动运行，每 {minutes} 分钟一次")))
        }
    }
}

pub fn remove_schedule() {
    unregister_task();
    remove_startup_links();
}

pub fn startup_installed() -> bool {
    startup_link().exists()
}
