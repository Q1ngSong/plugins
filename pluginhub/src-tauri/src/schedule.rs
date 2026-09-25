//! 后台任务：系统定时任务（schtasks），建不了就用「启动」文件夹里的快捷方式拉起开机自启的后台进程
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use fs2::FileExt;
use serde_json::{json, Map, Value};

use crate::bail;
use crate::store::{load_config, save_config, Config, ScheduleCfg};
use crate::util::*;

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

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn task_user() -> String {
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let user = std::env::var("USERNAME").unwrap_or_default();
    if domain.is_empty() { user } else { format!("{domain}\\{user}") }
}

/// 后台任务、开机自启用的命令：(程序, 参数, 工作目录)。后台跑的永远是这个 exe 自己。
pub fn launcher(args: &[&str]) -> (String, Vec<String>, String) {
    let exe = exe_path().unwrap_or_else(|| PathBuf::from("pluginhub.exe"));
    let workdir = exe.parent().map(display).unwrap_or_else(|| ".".into());
    let mut full = vec!["--run".to_string()];
    full.extend(args.iter().map(|s| s.to_string()));
    (display(&exe), full, workdir)
}

pub fn launcher_line(args: &[&str]) -> String {
    let (program, a, _) = launcher(args);
    let mut all = vec![program];
    all.extend(a);
    cmdline(&all)
}

fn schtasks(args: &[&str]) -> R<Out> {
    run("schtasks", args, None, 60, false, true)
}

pub fn register_task(minutes: i64) -> R<String> {
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

pub fn unregister_task() {
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
    m.insert("task_state".into(), Value::from(row.get(2).cloned().unwrap_or_default()));
    m
}

/// 非阻塞地锁住 daemon.lock。拿到就返回文件（锁跟着它，要一直拿着），拿不到返回 None。
pub fn try_daemon_lock() -> Option<fs::File> {
    fs::create_dir_all(&*HUB_DIR).ok()?;
    let f = fs::OpenOptions::new().read(true).append(true).create(true).open(&*DAEMON_LOCK).ok()?;
    if f.try_lock_exclusive().is_ok() { Some(f) } else { None }
}

pub fn daemon_alive() -> bool {
    match try_daemon_lock() {
        None => true,
        Some(f) => {
            let _ = f.unlock();
            false
        }
    }
}

pub fn start_daemon() -> R<()> {
    if daemon_alive() {
        return Ok(());
    }
    let (program, args, workdir) = launcher(&["daemon"]);
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    spawn_detached(Path::new(&program), &a, Some(Path::new(&workdir)))
}

/// 后台任务用的是系统定时任务（task）还是开机自启的后台进程（startup）。旧版记在 auto_update 里。
pub fn schedule_mode(cfg: &Config) -> Option<String> {
    cfg.schedule.as_ref().and_then(|s| s.mode.clone()).or_else(|| cfg.auto_update.mode.clone())
}

pub fn auto_minutes(cfg: &Config) -> i64 {
    cfg.auto_update.interval_minutes.clamp(15, 1440)
}

/// 后台任务多久跑一次：取开着的「后台检查」（每天）和「自动更新」里间隔短的那个；都没开返回 None
pub fn schedule_minutes(cfg: &Config) -> Option<i64> {
    let mut xs = Vec::new();
    if cfg.guard.enabled {
        xs.push(GUARD_MINUTES);
    }
    if cfg.auto_update.enabled {
        xs.push(auto_minutes(cfg));
    }
    xs.into_iter().min()
}

/// 离上次做过是否已经到了间隔（留一小时余量，免得每天的检查一点点往后拖）
pub fn due(last: Option<&str>, minutes: i64) -> bool {
    let Some(last) = last.filter(|s| !s.is_empty()) else { return true };
    match DateTime::parse_from_rfc3339(last) {
        Ok(t) => (Local::now().signed_duration_since(t)).num_seconds() >= minutes * 60 - 3600,
        Err(_) => true,
    }
}

pub fn startup_link() -> PathBuf {
    startup_folder().join(format!("{STARTUP_NAME}.lnk"))
}

/// 建快捷方式（PowerShell 调 WScript.Shell）；不行就退回到 .cmd 文件
pub fn make_link(lnk: &Path, program: &str, args: &[String], workdir: &str, description: &str) -> R<PathBuf> {
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

pub fn remove_startup_links() {
    let lnk = startup_link();
    let _ = fs::remove_file(&lnk);
    let _ = fs::remove_file(lnk.with_extension("cmd"));
}

/// 自动更新的设置，加上后台任务实际在不在跑、下次什么时候跑
pub fn auto_status(cfg: &Config) -> Value {
    let mut auto = serde_json::to_value(&cfg.auto_update).unwrap_or(json!({})).as_object().cloned().unwrap_or_default();
    let mode = schedule_mode(cfg);
    if mode.as_deref() == Some("startup") {
        let alive = daemon_alive();
        auto.insert("mode".into(), Value::from("startup"));
        auto.insert("installed".into(), Value::Bool(alive && startup_link().exists()));
        let next = if alive { read_obj(&DAEMON_INFO).get("next_run").and_then(Value::as_str).unwrap_or("").to_string() } else { String::new() };
        auto.insert("next_run".into(), Value::from(next));
        return Value::Object(auto);
    }
    let status = task_status();
    let installed = gb(&Value::Object(status.clone()), "installed");
    for (k, v) in status {
        auto.insert(k, v);
    }
    auto.insert("mode".into(), if installed { Value::from("task") } else { mode.map(Value::from).unwrap_or(Value::Null) });
    Value::Object(auto)
}

/// 按「后台检查」和「自动更新」的设置装上或卸掉后台任务，存好配置，返回说明。
///
/// 优先用系统定时任务；建不了（没有权限）就用「启动」文件夹里的快捷方式拉起开机自启的后台进程。
pub fn apply_schedule(cfg: &mut Config) -> R<String> {
    let minutes = schedule_minutes(cfg);
    cfg.auto_update.mode = None; // 旧版把方式记在这里，改记到 schedule
    let Some(minutes) = minutes else {
        unregister_task();
        remove_startup_links();
        cfg.schedule = Some(ScheduleCfg { mode: None });
        save_config(cfg)?; // 开机自启的后台进程每 30 秒看一次配置，看到关闭就自己退出
        return Ok("后台任务已停止".into());
    };
    let (mode, msg) = match register_task(minutes) {
        Ok(when) => ("task", format!("后台任务：{when}运行一次（每 {minutes} 分钟，系统定时任务）")),
        Err(e) => {
            log(&format!("系统定时任务建不了（{e}），改用开机自启的后台进程"));
            let (program, args, workdir) = launcher(&["daemon"]);
            make_link(&startup_link(), &program, &args, &workdir, "插件中心：后台检查插件配置、自动更新")?;
            ("startup", format!("后台任务：开机后自动运行，每 {minutes} 分钟一次"))
        }
    };
    cfg.schedule = Some(ScheduleCfg { mode: Some(mode.into()) });
    save_config(cfg)?;
    if mode == "startup" {
        start_daemon()?;
    }
    Ok(msg)
}

/// 桌面 App 里可能带着代理变量，定时任务里没有，记下来给 git 用
pub fn remember_proxy(cfg: &mut Config) {
    let mut proxy = std::collections::BTreeMap::new();
    for k in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "NO_PROXY", "https_proxy", "http_proxy", "all_proxy", "no_proxy"] {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                proxy.insert(k.to_string(), v);
            }
        }
    }
    if !proxy.is_empty() {
        cfg.proxy = Some(proxy);
    }
}

pub fn set_auto_update(enabled: bool, minutes: i64) -> R<Vec<String>> {
    let mut cfg = load_config();
    remember_proxy(&mut cfg);
    cfg.auto_update.enabled = enabled;
    cfg.auto_update.interval_minutes = minutes.clamp(15, 1440);
    let notes = vec![format!("已{}自动更新", if enabled { "开启" } else { "关闭" }), apply_schedule(&mut cfg)?];
    for n in &notes {
        log(n);
    }
    Ok(notes)
}

/// 后台任务走开机自启的后台进程时，如果它意外退出了，打开页面时顺手拉起来
pub fn ensure_daemon() {
    let cfg = load_config();
    if schedule_mode(&cfg).as_deref() == Some("startup") && schedule_minutes(&cfg).is_some() {
        if let Err(e) = start_daemon() {
            log(&format!("拉起后台进程失败：{e}"));
        }
    }
}
