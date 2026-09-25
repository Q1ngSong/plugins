//! 命令行模式：`pluginhub.exe --run <命令> [参数]`。定时任务、开机自启的后台进程也走这里，不开窗口。
use std::io::Write;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::bail;
use crate::ops;
use crate::schedule::*;
use crate::server::{self, Files};
use crate::store::*;
use crate::util::*;
use crate::view::build_state;

const USAGE: &str = "插件中心命令行：pluginhub.exe --run <命令>
  status                      在终端打印状态
  check                       检查有没有新版本（锁定的跳过），再做一遍真实检查
  update [插件] [--force]     拉取最新并同步到 Claude Code 和 Codex
  add <仓库> [--branch B] [--no-apply]  接管一个插件仓库
  guard                       马上检查两边的插件配置有没有被改掉，改掉了就修复
  auto                        后台任务入口：检查并修复配置，到点了就更新
  daemon                      开机自启的后台进程
  install [--interval 分钟]   开启自动更新（后台任务）
  uninstall                   关掉后台检查、自动更新和后台任务
  serve [--port N] [--no-browser]  在浏览器里用（页面服务）
  api <接口> [JSON]           直接调用页面用的接口，输出 JSON（脚本和测试用）";

fn out(s: &str) {
    let _ = writeln!(std::io::stdout(), "{s}");
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn value_of(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a == "--branch" || a == "--port" || a == "--interval" || a == "--owner" {
            skip = true;
            continue;
        }
        if !a.starts_with("--") {
            out.push(a.clone());
        }
    }
    out
}

fn print_notes(notes: &[String], empty: &str) {
    if notes.is_empty() {
        out(empty);
    } else {
        out(&notes.join("\n"));
    }
}

fn cmd_status() -> R<()> {
    let s = build_state()?;
    let hub = &s["hub"];
    let auto = &hub["auto"];
    out(&format!("插件中心 {HUB_VERSION}  配置目录 {}", display(&HUB_DIR)));
    out(&format!("claude.exe: {}", if gs(&s["tools"], "claude").is_empty() { "没找到" } else { gs(&s["tools"], "claude") }));
    out(&format!("codex.exe:  {}", if gs(&s["tools"], "codex").is_empty() { "没找到" } else { gs(&s["tools"], "codex") }));
    let auto_text = if gb(auto, "enabled") { format!("开（每 {} 分钟）", auto.get("interval_minutes").and_then(Value::as_i64).unwrap_or(60)) } else { "关".into() };
    let task_text = if gb(auto, "installed") { format!("在运行，下次 {}", gs(auto, "next_run")) } else { "没有".into() };
    out(&format!("自动更新：{auto_text}  后台检查：{}  后台任务：{task_text}", if gb(&hub["guard"], "enabled") { "开" } else { "关" }));
    out(&format!("后台任务的命令：{}", gs(hub, "launcher")));
    let lib = &s["library"];
    if gb(lib, "on") {
        let num = |k: &str| lib.get(k).and_then(Value::as_u64).unwrap_or(0);
        out(&format!("技能库：按需的 {} 个技能在 {}，每次会话只占约 {} token，省下约 {} token", num("skills"), gs(lib, "folder"), num("cost"), num("saved")));
        for a in ga(lib, "apps") {
            for problem in ga(a, "problems").iter().filter_map(Value::as_str) {
                out(&format!("   {} ! {problem}", app_name(gs(a, "app"))));
            }
        }
    }
    for r in ga(&s, "plugins") {
        let v = r.get("managed").filter(|m| m.is_object());
        let head = match v {
            Some(v) => format!("[{}] {}", gs(v, "branch"), gs(v, "repo")),
            None => "（未接管）".into(),
        };
        out(&format!("\n== {}  {head}", gs(r, "name")));
        if gb(r, "on_demand") {
            out("   按需：没装进 app，列在技能库里，用到时再读");
        }
        if let Some(v) = v {
            if let Some(remote) = v.get("remote").filter(|x| x.is_object()) {
                out(&format!("   远端最新：{}  {}", gs(remote, "short"), gs(remote, "subject")));
            }
            if gb(v, "locked") {
                out("   已锁定：不检查、不拉取更新");
            }
            let modified: Vec<&str> = ga(v, "modified").iter().filter_map(Value::as_str).collect();
            if !modified.is_empty() {
                out(&format!("   插件文件夹里有修改：{}", modified.join("、")));
            }
            if !gs(v, "error").is_empty() {
                out(&format!("   上次出错：{}", gs(v, "error")));
            }
        }
        for a in ga(r, "apps") {
            let state = if gs(a, "state").is_empty() { if gb(a, "enabled") { "启用" } else { "停用" } } else { gs(a, "state") };
            let version = a.get("version").map(|v| match v { Value::String(s) => s.clone(), other => other.to_string() }).unwrap_or_else(|| "null".into());
            out(&format!("   {:<12} {:<48} {:<9} 版本 {version}", app_name(gs(a, "app")), gs(a, "id"), state));
            for problem in ga(a, "problems").iter().filter_map(Value::as_str) {
                out(&format!("      ! {problem}"));
            }
            if let Some(v) = a.get("verify").filter(|x| x.is_object()) {
                out(&format!("      真实检查{}：{}", if gb(v, "ok") { "通过" } else { "没通过" }, gs(v, "detail")));
            }
            for x in ga(a, "projects") {
                out(&format!("      {}  调用 {} 次", gs(x, "path"), x.get("count").and_then(Value::as_u64).unwrap_or(0)));
            }
        }
    }
    Ok(())
}

/// 后台任务入口（系统定时任务或开机自启的后台进程每次调用）：检查并修复配置，到点了就更新
fn cmd_auto() {
    let result = hub_lock(30).and_then(|_lock| ops::tick());
    if let Err(e) = result {
        // 后台任务没有界面，出错只能写日志
        log(&format!("后台任务出错：{e}"));
    }
}

/// 开机自启的后台进程：按设置的间隔跑后台任务；后台检查和自动更新都关掉了就退出
fn cmd_daemon() -> R<()> {
    let Some(_lock) = try_daemon_lock() else { return Ok(()) }; // 拿不到锁：已经有一个在跑
    log("后台进程已启动");
    let mut next_run = Instant::now() + Duration::from_secs(60); // 登录后等一分钟再查，不和开机抢资源
    let mut next_at = chrono::Local::now() + chrono::Duration::seconds(60);
    let mut shown = String::new();
    loop {
        let cfg = load_config();
        let minutes = schedule_minutes(&cfg);
        if minutes.is_none() || schedule_mode(&cfg).as_deref() != Some("startup") {
            log("后台任务已关闭，后台进程退出");
            let _ = std::fs::remove_file(&*DAEMON_INFO);
            return Ok(());
        }
        if Instant::now() >= next_run {
            cmd_auto();
            let secs = minutes.unwrap_or(GUARD_MINUTES) * 60;
            next_run = Instant::now() + Duration::from_secs(secs as u64);
            next_at = chrono::Local::now() + chrono::Duration::seconds(secs);
        }
        let label = next_at.format("%m-%d %H:%M").to_string();
        if label != shown {
            let _ = write_json(&DAEMON_INFO, &json!({"pid": std::process::id(), "next_run": label}));
            shown = label;
        }
        std::thread::sleep(Duration::from_secs(30));
    }
}

fn cmd_uninstall() -> R<()> {
    let mut cfg = load_config();
    cfg.auto_update.enabled = false;
    cfg.guard.enabled = false;
    apply_schedule(&mut cfg)?; // 删掉定时任务或开机自启；后台进程看到关闭会自己退出
    log("关掉了后台检查、自动更新和后台任务");
    out(&format!("已关掉后台检查、自动更新和后台任务。配置和插件还在 {}、{}，不需要可以手动删除。", display(&HUB_DIR), display(&PLUGINS_DIR)));
    Ok(())
}

fn cmd_api(rest: &[String]) -> R<()> {
    let Some(name) = rest.first() else { bail!("要写接口名，比如 --run api state") };
    let body: Value = match rest.get(1) {
        Some(text) => serde_json::from_str(text).map_err(|e| HubError::Msg(format!("JSON 参数不对：{e}")))?,
        None => json!({}),
    };
    match ops::dispatch(name, &body) {
        Ok(v) => {
            out(&v.to_string());
            Ok(())
        }
        Err((code, e)) => {
            out(&e.to_string());
            bail!("接口 {name} 失败（{code}）：{}", gs(&e, "error"))
        }
    }
}

pub fn run_cli(args: &[String], files: Files) -> i32 {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    let pos = positional(&rest);
    let result: R<()> = match cmd {
        "status" => cmd_status(),
        "check" => hub_lock(120).and_then(|_l| ops::op_check()).map(|n| print_notes(&n, "没有新版本")),
        "update" => hub_lock(120).and_then(|_l| ops::op_update(pos.first().map(String::as_str), flag(&rest, "--force"))).map(|n| print_notes(&n, "都已是最新")),
        "add" => match pos.first() {
            Some(repo) => hub_lock(120)
                .and_then(|_l| ops::add_plugin(repo, &value_of(&rest, "--branch").unwrap_or_default(), !flag(&rest, "--no-apply"), &["claude".into(), "codex".into()]))
                .map(|n| print_notes(&n, "")),
            None => Err(HubError::Msg("要写仓库地址。".into())),
        },
        "guard" => hub_lock(120).and_then(|_l| ops::op_guard()).map(|n| print_notes(&n, "两边的插件配置都没问题")),
        "auto" => {
            cmd_auto();
            Ok(())
        }
        "daemon" => cmd_daemon(),
        "install" => {
            let cfg = load_config();
            let minutes = value_of(&rest, "--interval").and_then(|v| v.parse().ok()).unwrap_or(cfg.auto_update.interval_minutes);
            set_auto_update(true, minutes).map(|n| print_notes(&n, ""))
        }
        "uninstall" => cmd_uninstall(),
        "serve" => {
            let port = value_of(&rest, "--port").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_PORT);
            let owner = value_of(&rest, "--owner").and_then(|v| v.parse().ok());
            ensure_daemon();
            server::serve(files, port, !flag(&rest, "--no-browser"), owner)
        }
        "api" => cmd_api(&rest),
        "help" | "--help" | "-h" | "" => {
            out(USAGE);
            Ok(())
        }
        other => Err(HubError::Msg(format!("不认识的命令：{other}\n{USAGE}"))),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            log(&format!("命令 {cmd} 失败：{e}"));
            let _ = writeln!(std::io::stderr(), "错误：{e}");
            1
        }
    }
}
