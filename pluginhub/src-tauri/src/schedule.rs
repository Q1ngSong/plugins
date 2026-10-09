//! 后台任务：按「后台检查」和「自动更新」的设置，让系统每隔一段时间运行一次 `--run auto`。
//! 系统任务怎么建由平台决定（见 platform/：Windows 是计划任务，建不了退到「启动」文件夹的快捷方式拉起常驻的后台进程；
//! macOS 是 launchd）。这里管设置、状态，以及常驻后台进程的锁。
//!
//! 常驻的后台进程跟着程序走：建快捷方式时把程序的路径和版本登记在 config.json 的 schedule 里，
//! 后台进程把自己的路径和版本写在 daemon.json 里。打开程序时登记的不是它，就改登记、改快捷方式；
//! 在跑的后台进程看到登记换了就退出，新的等它让位再接手。
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use serde_json::{json, Value};

use crate::platform;
use crate::store::{load_config, load_state, save_config_with_schedule, Config, ScheduleCfg};
use crate::util::*;

/// 等在跑的后台进程让位，最多等这么久。它每 30 秒看一次配置，但正在跑后台任务的话要等这一轮跑完（断网时会拖很久），
/// 所以放宽到一小时：等的这边先走了，它跑完照样会让位，就一个都不剩了
const TAKEOVER_SECS: u64 = 3600;
/// 在跑的是 1.4.3 及以前的后台进程（daemon.json 里没写自己是哪个程序）：它不认登记，不会让位，只等这么久
const LEGACY_SECS: u64 = 60;

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

/// 这个程序自己的路径（和写进快捷方式的一样）和版本
fn me() -> (String, &'static str) {
    (launcher(&["daemon"]).0, HUB_VERSION)
}

/// 日志里写一个程序：路径（版本）
fn program_label(exe: &str, version: &str) -> String {
    format!("{exe}（{version}）")
}

/// 这个程序自己在日志里的写法
pub fn this_program() -> String {
    let (exe, version) = me();
    program_label(&exe, version)
}

/// 开机自启这种方式下，后台任务登记给了哪个程序：(路径, 版本)。
/// 没登记过返回 None：1.4.3 及以前建的，或者后台任务用的不是这种方式
fn registered(cfg: &Config) -> Option<(&str, &str)> {
    let s = cfg.schedule.as_ref()?;
    Some((s.exe.as_deref()?, s.version.as_deref()?))
}

/// (exe, version) 这个后台进程该不该给登记的 by 让位：登记的是另一个位置的程序，或者同一个位置登记了更新的版本
/// （自己是更新前留在内存里的那个）。登记的版本比自己旧不让：程序更新后还没打开过，登记还是老的。
/// 路径解析掉链接和短文件名再比，免得同一个文件的两种写法被当成两个程序
fn superseded(exe: &str, version: &str, by: (&str, &str)) -> bool {
    !same_real(Path::new(exe), Path::new(by.0)) || version_key(by.1) > version_key(version)
}

/// 后台任务是不是登记给了别的程序（见 superseded）。是的话返回登记的那个，后台进程看到就退出
pub fn handed_over(cfg: &Config) -> Option<String> {
    let (exe, version) = me();
    registered(cfg).filter(|by| superseded(&exe, version, *by)).map(|(e, v)| program_label(e, v))
}

/// 后台进程把自己记到 daemon.json：进程号、下次什么时候跑、程序的路径和版本
pub fn write_daemon_info(next_run: &str) {
    let (exe, version) = me();
    let _ = write_json(&DAEMON_INFO, &json!({"pid": std::process::id(), "next_run": next_run, "exe": exe, "version": version}));
}

/// 在跑的后台进程是不是这个程序（看它写在 daemon.json 里的路径和版本）。旧版本的后台进程不写这两项，算不是
fn running_is_me() -> bool {
    let info = Value::Object(read_obj(&DAEMON_INFO));
    let (exe, version) = me();
    same_real(Path::new(gs(&info, "exe")), Path::new(&exe)) && gs(&info, "version") == version
}

/// 后台进程启动时拿锁。锁被占着：在跑的就是这个程序，或者后台任务没登记给这个程序，就不用起，返回 None；
/// 登记的是这个程序、在跑的却是另一个（别的位置的，或者更新前留下的旧版本），就等它让位再接手
pub fn daemon_lock() -> Option<fs::File> {
    let started = Instant::now();
    let (exe, version) = me();
    let mut round = 0;
    loop {
        if let Some(f) = try_daemon_lock() {
            return Some(f);
        }
        let mine = registered(&load_config()).is_some_and(|by| !superseded(&exe, version, by));
        if !mine || running_is_me() {
            return None;
        }
        let info = Value::Object(read_obj(&DAEMON_INFO));
        let pid = info.get("pid").and_then(Value::as_u64).unwrap_or(0);
        let legacy = gs(&info, "exe").is_empty();
        if started.elapsed() >= Duration::from_secs(if legacy { LEGACY_SECS } else { TAKEOVER_SECS }) {
            // 快捷方式已经改过来了，重新登录后跑的就是这个程序
            let why = if legacy { "是旧版本的，不认登记、不会让位" } else { "一直没有让位" };
            log(&format!("在跑的后台进程（进程号 {pid}）{why}，这次不接手；重新登录后换成 {}", program_label(&exe, version)));
            return None;
        }
        // 头一轮先不说：刚起来的后台进程可能还没来得及写 daemon.json
        if round == 1 {
            log(&format!("在跑的后台进程（进程号 {pid}）不是 {}，等它让位", program_label(&exe, version)));
        }
        round += 1;
        std::thread::sleep(Duration::from_secs(2));
    }
}

pub fn start_daemon() -> R<()> {
    // 在跑的是别的程序或别的版本，也照样拉起自己的：它等那一个让位再接手（daemon_lock）
    if daemon_alive() && running_is_me() {
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
        cfg.schedule = Some(ScheduleCfg::default());
        save_config_with_schedule(cfg)?; // 常驻的后台进程每 30 秒看一次配置，看到关闭就自己退出
        return Ok("后台任务已停止".into());
    };
    let (mode, msg) = platform::install_schedule(minutes)?;
    // 开机自启：快捷方式指向这个程序，把它登记下来；在跑的后台进程是别的程序的话，看到登记换了会退出
    let (exe, version) = if mode == "startup" { (Some(me().0), Some(HUB_VERSION.to_string())) } else { (None, None) };
    cfg.schedule = Some(ScheduleCfg { mode: Some(mode.into()), exe, version });
    save_config_with_schedule(cfg)?;
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

/// 打开页面时顺手看一眼后台任务：常驻的后台进程意外退出了就拉起来，在跑的不是这个程序就换成这个程序；
/// 程序挪了位置（比如从下载文件夹拖进了「应用程序」）、系统任务还指着已经不存在的老路径，就重新登记一遍
pub fn ensure_background() {
    let mut cfg = load_config();
    if schedule_minutes(&cfg).is_none() {
        return;
    }
    match schedule_mode(&cfg).as_deref() {
        Some("startup") => {
            let (exe, version) = me();
            let same_place = registered(&cfg).is_some_and(|(e, _)| same_real(Path::new(e), Path::new(&exe)));
            let same_version = registered(&cfg).is_some_and(|(_, v)| v == version);
            let result = if same_place && same_version {
                start_daemon()
            } else if same_place {
                // 位置没变，只是版本不同（刚更新过）：快捷方式不用动，把登记的版本改过来
                cfg.schedule.get_or_insert_default().version = Some(version.to_string());
                save_config_with_schedule(&cfg).and_then(|_| start_daemon())
            } else {
                // 登记的是别的位置的程序（重装到了别处，或者后台任务是另一份程序开的），或者是旧版本建的、没有登记：
                // 重新登记，快捷方式改成指向这个程序。快捷方式指向谁只看登记，不去读它（要调 PowerShell，慢）
                let was = registered(&cfg).map(|(e, v)| format!("登记的是 {}", program_label(e, v))).unwrap_or_else(|| "没有登记".into());
                apply_schedule(&mut cfg).map(|msg| log(&format!("后台任务换成这个程序来跑：{}，原来{was}。{msg}", program_label(&exe, version))))
            };
            if let Err(e) = result {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_yields_to_another_place_or_a_newer_version() {
        let here = display(&HOME.join("hub").join(platform::EXE_NAME));
        let there = display(&HOME.join("hub-dev").join(platform::EXE_NAME));
        assert!(!superseded(&here, "1.4.4", (&here, "1.4.4"))); // 登记的就是自己
        assert!(superseded(&here, "1.4.4", (&there, "1.4.4"))); // 登记给了别的位置的程序
        assert!(superseded(&here, "1.4.9", (&here, "1.4.10"))); // 同一个位置登记了更新的版本：自己是更新前留下的
        assert!(!superseded(&here, "1.4.4", (&here, "1.4.3"))); // 更新后还没打开过，登记还是老的
    }

    #[test]
    fn registration_is_optional_in_config() {
        let mut cfg = Config::default();
        assert!(registered(&cfg).is_none()); // 没开后台任务
        cfg.schedule = Some(ScheduleCfg { mode: Some("startup".into()), ..Default::default() });
        assert!(registered(&cfg).is_none()); // 1.4.3 及以前建的：只有方式，没登记程序
        cfg.schedule = Some(ScheduleCfg { mode: Some("startup".into()), exe: Some("x".into()), version: Some("1.4.4".into()) });
        assert_eq!(registered(&cfg), Some(("x", "1.4.4")));
        // 没登记的配置存回去不多出字段（macOS 和系统任务的配置和以前一样）
        let saved = serde_json::to_value(ScheduleCfg { mode: Some("task".into()), ..Default::default() }).unwrap();
        assert_eq!(saved, json!({"mode": "task"}));
    }
}
