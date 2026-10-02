//! 插件中心：统一管理 Claude Code 和 Codex 的插件。
//!
//! 每个受管插件在 ~/.yuwanplugins/<名字> 有一份 git 克隆，它是唯一存放处，两边都链接过来：
//! - Claude Code：~/.claude/skills/<名字> 是指向它的目录链接，Claude Code 就地加载成 <名字>@skills-dir。
//! - Codex：个人插件源指向它；Codex 只认真实的缓存文件夹，所以正常装一份后，
//!   把缓存里的子文件夹换成指向它的链接（清单文件照原样复制）。
//! 拉取后两边直接用上，不用重装；两边的"版本"就是这份克隆的提交号。
//! 不要在 ~/.yuwanplugins 里改插件：有修改时同步会停下来，提示锁定或另存。
//!
//! 不带参数运行打开窗口；`--run <命令>` 是命令行模式（见 cli.rs），定时任务和开机自启也走它。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod claude;
mod cli;
mod codex;
mod gitx;
mod ops;
mod schedule;
mod server;
mod skills;
mod store;
mod usage;
mod util;
mod view;

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::utils::assets::AssetKey;
use tauri::{WebviewUrl, WebviewWindowBuilder};

/// 页面调用的唯一接口：name 是接口名（state、check、update…），body 是参数。
/// 出错时返回 {"error": 说明, "state": 最新状态}。
#[tauri::command]
async fn api(name: String, body: Option<Value>) -> Result<Value, Value> {
    let body = body.unwrap_or_else(|| json!({}));
    match tauri::async_runtime::spawn_blocking(move || ops::dispatch(&name, &body)).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err((_, e))) => Err(e),
        Err(e) => Err(json!({"error": format!("内部错误：{e}")})),
    }
}

/// exe 里带的页面文件（浏览器版用）
fn embedded_files(context: &tauri::Context<tauri::Wry>) -> server::Files {
    let assets = context.assets();
    let mut files = HashMap::new();
    for (key, _) in assets.iter() {
        if let Some(data) = assets.get(&AssetKey::from(key.as_ref())) {
            files.insert(key.to_string(), data.into_owned());
        }
    }
    Arc::new(files)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let context: tauri::Context<tauri::Wry> = tauri::generate_context!();
    if args.get(1).map(String::as_str) == Some("--run") {
        // 常驻的后台进程不接控制台：接上了，关掉那个终端窗口它就会被一起关掉
        if !matches!(args.get(2).map(String::as_str), Some("daemon" | "auto")) {
            util::attach_console();
        }
        let files = embedded_files(&context);
        std::process::exit(cli::run_cli(&args[2..], files));
    }
    schedule::ensure_daemon();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![api])
        .setup(|app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("插件中心")
                .inner_size(1000.0, 760.0)
                .min_inner_size(640.0, 520.0)
                .center()
                .build()?;
            Ok(())
        })
        .run(context)
        .expect("插件中心启动失败");
}
