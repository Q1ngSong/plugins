//! 浏览器版的页面服务：只绑定 127.0.0.1，校验 Host 头（防 DNS 重绑定），接口要带页面里嵌的随机令牌。
//! 页面文件从 exe 里带的那份（和窗口版同一份）提供。
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::bail;
use crate::ops::dispatch;
use crate::util::*;

pub type Files = Arc<HashMap<String, Vec<u8>>>;

fn content_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next().unwrap_or("");
    Some(match ext {
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "html" => "text/html; charset=utf-8",
        _ => return None,
    })
}

struct Server {
    token: String,
    files: Files,
    last_seen: Mutex<Instant>,
}

fn respond(stream: &mut TcpStream, code: u16, body: &[u8], ctype: &str) {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Internal Server Error",
    };
    let head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn respond_json(stream: &mut TcpStream, code: u16, v: &Value) {
    respond(stream, code, v.to_string().as_bytes(), "application/json; charset=utf-8");
}

fn handle(mut stream: TcpStream, srv: &Server) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let path = target.split('?').next().unwrap_or("/").to_string();
    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; length.min(4 << 20)];
    if !body.is_empty() && reader.read_exact(&mut body).is_err() {
        return;
    }
    if method == "GET" && path == "/api/ping" {
        return respond_json(&mut stream, 200, &json!({"app": "pluginhub", "version": HUB_VERSION}));
    }
    // 只接受本机地址（防 DNS 重绑定），接口还要带页面里的令牌（防别的网页乱调）
    let host = headers.get("host").cloned().unwrap_or_default();
    let host = host.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or(host);
    let host = host.trim_matches(|c| c == '[' || c == ']');
    if host != "127.0.0.1" && host != "localhost" {
        return respond_json(&mut stream, 403, &json!({"error": "forbidden"}));
    }
    if path.starts_with("/api/") && headers.get("x-hub-token").map(String::as_str) != Some(srv.token.as_str()) {
        return respond_json(&mut stream, 403, &json!({"error": "页面已过期，请刷新。"}));
    }
    *srv.last_seen.lock().expect("lock") = Instant::now();
    if method == "GET" {
        if path == "/" || path == "/index.html" {
            return match srv.files.get("/index.html") {
                Some(html) => {
                    let text = String::from_utf8_lossy(html).replace("__HUB_TOKEN__", &srv.token);
                    respond(&mut stream, 200, text.as_bytes(), "text/html; charset=utf-8")
                }
                None => respond(&mut stream, 500, "页面文件缺失，请重新安装插件中心。".as_bytes(), "text/plain; charset=utf-8"),
            };
        }
        if path == "/favicon.ico" {
            return respond(&mut stream, 200, include_bytes!("../icons/icon.ico"), "image/x-icon");
        }
        if let (Some(data), Some(ct)) = (srv.files.get(&path), content_type(&path)) {
            return respond(&mut stream, 200, data, ct);
        }
        if path == "/api/state" {
            return match dispatch("state", &Value::Null) {
                Ok(v) => respond_json(&mut stream, 200, &v),
                Err((code, e)) => respond_json(&mut stream, code, &e),
            };
        }
        return respond_json(&mut stream, 404, &json!({"error": "not found"}));
    }
    if method != "POST" || !path.starts_with("/api/") {
        return respond_json(&mut stream, 404, &json!({"error": "not found"}));
    }
    let body: Value = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return respond_json(&mut stream, 400, &json!({"error": "请求格式不对"})),
        }
    };
    let name = &path[5..];
    if name == "shutdown" {
        respond_json(&mut stream, 200, &json!({"notes": ["管理页面服务已退出"]}));
        log("管理页面服务退出");
        cleanup_info();
        std::process::exit(0);
    }
    match dispatch(name, &body) {
        Ok(v) => respond_json(&mut stream, 200, &v),
        Err((code, e)) => respond_json(&mut stream, code, &e),
    }
}

fn cleanup_info() {
    let mine = read_obj(&SERVER_INFO).get("pid").and_then(Value::as_u64) == Some(std::process::id() as u64);
    if mine {
        let _ = std::fs::remove_file(&*SERVER_INFO);
    }
}

pub fn pick_port(preferred: u16) -> R<u16> {
    for port in preferred..preferred.saturating_add(20) {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    bail!("找不到可用的本地端口。")
}


/// 前台运行页面服务。owner 是桌面窗口的进程号：它退出时服务也退出；没有 owner 时闲置太久自动退出。
pub fn serve(files: Files, port: u16, open_browser: bool, owner: Option<u32>) -> R<()> {
    let port = pick_port(port)?;
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let srv = Arc::new(Server { token, files, last_seen: Mutex::new(Instant::now()) });
    let url = format!("http://127.0.0.1:{port}/");
    write_json(&SERVER_INFO, &json!({"port": port, "pid": std::process::id(), "started": stamp()}))?;
    log(&format!("管理页面已启动：{url}"));
    let _ = writeln!(std::io::stdout(), "插件中心：{url}");
    let watch = srv.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(if owner.is_some() { 5 } else { 60 }));
        let idle = watch.last_seen.lock().expect("lock").elapsed();
        if let Some(pid) = owner {
            if !process_alive(pid) {
                log("插件中心窗口关掉了，页面服务退出");
                cleanup_info();
                std::process::exit(0);
            }
        } else if idle > Duration::from_secs(IDLE_EXIT_MINUTES * 60) {
            log("管理页面闲置太久，服务自动退出");
            cleanup_info();
            std::process::exit(0);
        }
    });
    if open_browser {
        let _ = shell_open(&url);
    }
    for stream in listener.incoming().flatten() {
        let srv = srv.clone();
        std::thread::spawn(move || handle(stream, &srv));
    }
    Ok(())
}
