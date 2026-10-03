//! 后台任务：按「后台检查」和「自动更新」的设置，让系统每隔一段时间运行一次 `--run auto`。
//! 系统任务怎么建由平台决定（见 platform/：Windows 是计划任务，建不了退到「启动」文件夹的快捷方式拉起常驻的后台进程；
//! macOS 是 launchd）。这里管设置、状态，以及常驻后台进程的锁。
use std::fs;
use std::path::Path;

use chrono::{DateTime, Local};
use serde_json::{json, Value};

use crate::platform;
use crate::store::{load_config, load_state, save_config, Config, ScheduleCfg};
use crate::util::*;

/// 非阻塞地锁住 daemon.lock。拿到就返回文件（锁跟着它，要一直拿着），拿不到返回 None。
pub fn try_daemon_lock() -> Option<fs::File> {
    fs::create_dir_all(&*HUB_DIR).ok()?;
    let f = fs::OpenOptions::new().read(true).append(true).create(true).open(&*DAEMON_LOCK).ok()?;
    if f.try_lock().is_ok() { Some(f) } else { None }
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
    platform::spawn_detached(Path::new(&program), &a, Some(Path::new(&workdir)))
}

/// 后台任务用的是系统任务（task）还是开机自启的后台进程（startup）。旧版记在 auto_update 里。
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

/// 系统不报下次什么时候跑的（launchd）：按上次跑过的时间加间隔算。后台任务每次跑完把时间记在 state.json 的 last_tick
fn next_from_last_tick(cfg: &Config) -> String {
    let Some(minutes) = schedule_minutes(cfg) else { return String::new() };
    let st = load_state();
    let Some(last) = st.get("last_tick").and_then(Value::as_str) else { return String::new() };
    match DateTime::parse_from_rfc3339(last) {
        Ok(t) => (t.with_timezone(&Local) + chrono::Duration::minutes(minutes)).format("%m-%d %H:%M").to_string(),
        Err(_) => String::new(),
    }
}

/// 自动更新的设置，加上后台任务实际在不在跑、下次什么时候跑
pub fn auto_status(cfg: &Config) -> Value {
    let mut auto = serde_json::to_value(&cfg.auto_update).unwrap_or(json!({})).as_object().cloned().unwrap_or_default();
    let mode = schedule_mode(cfg);
    if mode.as_deref() == Some("startup") {
        let alive = daemon_alive();
        auto.insert("mode".into(), Value::from("startup"));
        auto.insert("installed".into(), Value::Bool(alive && platform::startup_installed()));
        let next = if alive { read_obj(&DAEMON_INFO).get("next_run").and_then(Value::as_str).unwrap_or("").to_string() } else { String::new() };
        auto.insert("next_run".into(), Value::from(next));
        return Value::Object(auto);
    }
    let status = platform::task_status();
    let installed = gb(&Value::Object(status.clone()), "installed");
    for (k, v) in status {
        auto.insert(k, v);
    }
    if installed && auto.get("next_run").and_then(Value::as_str).unwrap_or("").is_empty() {
        auto.insert("next_run".into(), Value::from(next_from_last_tick(cfg)));
    }
    auto.insert("mode".into(), if installed { Value::from("task") } else { mode.map(Value::from).unwrap_or(Value::Null) });
    Value::Object(auto)
}

/// 按「后台检查」和「自动更新」的设置装上或卸掉后台任务，存好配置，返回说明。
pub fn apply_schedule(cfg: &mut Config) -> R<String> {
    let minutes = schedule_minutes(cfg);
    cfg.auto_update.mode = None; // 旧版把方式记在这里，改记到 schedule
    let Some(minutes) = minutes else {
        platform::remove_schedule();
        cfg.schedule = Some(ScheduleCfg { mode: None });
        save_config(cfg)?; // 常驻的后台进程每 30 秒看一次配置，看到关闭就自己退出
        return Ok("后台任务已停止".into());
    };
    let (mode, msg) = platform::install_schedule(minutes)?;
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

/// 打开页面时顺手看一眼后台任务：常驻的后台进程意外退出了就拉起来；
/// 程序挪了位置（比如从下载文件夹拖进了「应用程序」）、系统任务还指着已经不存在的老路径，就重新登记一遍
pub fn ensure_background() {
    let mut cfg = load_config();
    if schedule_minutes(&cfg).is_none() {
        return;
    }
    match schedule_mode(&cfg).as_deref() {
        Some("startup") => {
            if let Err(e) = start_daemon() {
                log(&format!("拉起后台进程失败：{e}"));
            }
        }
        Some("task") => {
            let status = Value::Object(platform::task_status());
            let program = gs(&status, "program");
            if gb(&status, "installed") && !program.is_empty() && !Path::new(program).exists() {
                match apply_schedule(&mut cfg) {
                    Ok(msg) => log(&format!("程序换了位置，重新登记后台任务。{msg}")),
                    Err(e) => log(&format!("程序换了位置，重新登记后台任务失败：{e}")),
                }
            }
        }
        _ => {}
    }
}
