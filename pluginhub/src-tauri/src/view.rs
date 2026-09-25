//! 状态汇总：本机所有插件、受管插件的来源信息、两边各自的状态、用过它的项目；移植 skill
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::bail;
use crate::claude::{describe_claude_source, split_id, ClaudeCode};
use crate::codex::Codex;
use crate::gitx::*;
use crate::schedule::{auto_status, launcher_line};
use crate::store::*;
use crate::usage::{merge_projects, project_rows, scan_usage, ProjRow, Usage};
use crate::util::*;

/// 受管插件的来源信息：跟踪的分支、本机和远端的提交、两边各自的状态
pub fn plugin_view(p: &PluginCfg, st: &mut State, claude: &ClaudeCode, codex: &Codex) -> Value {
    let m = memo(st, &p.id).clone();
    let locked = p.locked;
    let mut view = json!({
        "id": p.id, "repo": p.repo, "web": repo_web_url(&p.repo), "slug": repo_slug(&p.repo),
        "branch": p.branch, "checked_at": m.get("checked_at").cloned().unwrap_or(Value::Null),
        "updated_at": m.get("updated_at").cloned().unwrap_or(Value::Null),
        "error": m.get("error").cloned().unwrap_or(Value::Null), "cloned": false, "needs_update": true,
        "claude": [], "codex": null, "locked": locked, "modified": [],
    });
    let d = repo_dir(p);
    if !d.join(".git").exists() {
        return view;
    }
    let head = commit_info(&d, "HEAD");
    let remote = commit_info(&d, &format!("origin/{}", p.branch)).or_else(|| head.clone());
    let man = read_manifests(&d, None);
    let head_sha = head.as_ref().map(|c| c.sha.clone());
    // 锁定的插件不看远端有没有新版本
    let latest = if locked { head_sha.clone() } else { remote.as_ref().map(|c| c.sha.clone()).or_else(|| head_sha.clone()) };
    let behind = matches!((&latest, &head_sha), (Some(l), Some(h)) if l != h);
    let synced = m.get("commit").and_then(Value::as_str).map(str::to_string);
    let obj = view.as_object_mut().expect("object");
    obj.insert("cloned".into(), Value::Bool(true));
    obj.insert("head".into(), serde_json::to_value(&head).unwrap_or(Value::Null));
    obj.insert("remote".into(), serde_json::to_value(&remote).unwrap_or(Value::Null));
    obj.insert("manifest".into(), serde_json::to_value(&man).unwrap_or(Value::Null));
    obj.insert("branches".into(), json!(remote_branches(&d)));
    obj.insert("behind".into(), Value::Bool(behind));
    obj.insert("modified".into(), json!(local_changes(&d, &p.branch, synced.as_deref())));
    let mut targets: Vec<Value> = Vec::new();
    if p.target("claude") {
        let list: Vec<Value> = man.claude_plugins.iter().map(|c| claude.status(&c.name, &d.join(&c.path), head_sha.as_deref(), latest.as_deref())).collect();
        targets.extend(list.iter().cloned());
        obj.insert("claude".into(), Value::Array(list));
    }
    if p.target("codex") {
        if let Some(name) = &man.codex_name {
            let s = codex.status(name, &d, head_sha.as_deref(), latest.as_deref());
            targets.push(s.clone());
            obj.insert("codex".into(), s);
        }
    }
    let needs = behind || targets.iter().any(|t| gs(t, "state") != "ok");
    obj.insert("needs_update".into(), Value::Bool(needs));
    view
}

fn merge_into(a: Map<String, Value>, b: Option<&Value>) -> Value {
    let mut out = a;
    if let Some(Value::Object(m)) = b {
        for (k, v) in m {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// 一个 Claude Code 插件在本机的情况，以及用过它的项目
pub fn claude_app(claude: &ClaudeCode, pid: &str, usage: &Usage, status: Option<&Value>) -> Value {
    let (name, mkt) = split_id(pid);
    let entries = ClaudeCode::installed().get(pid).and_then(Value::as_array).cloned().unwrap_or_default();
    let e = claude.entry(pid).unwrap_or(Value::Null);
    let mut projects = merge_projects(&[usage.get("claude", name)]);
    for x in &entries {
        // 装在项目范围的，那个项目自然算在用
        let pp = gs(x, "projectPath");
        if !pp.is_empty() && Path::new(pp).is_dir() {
            let row = projects.entry(norm(Path::new(pp))).or_insert_with(|| ProjRow { path: pp.to_string(), count: 0, last: 0.0, scope: None });
            row.scope = x.get("scope").and_then(Value::as_str).map(str::to_string);
        }
    }
    let (path, pj, installed, version, enabled, source): (Option<PathBuf>, Map<String, Value>, bool, Value, Value, String) = if mkt == "skills-dir" {
        // 就地加载的：没有安装记录，看 ~/.claude/skills 里的文件夹
        let path = CLAUDE_SKILLS.join(name);
        let pj = read_obj(&path.join(".claude-plugin").join("plugin.json"));
        let version = pj.get("version").cloned().unwrap_or(Value::Null);
        let source = format!("就地加载 {}", display(&path));
        (Some(path), pj, true, version, Value::Bool(ClaudeCode::enabled_unless_false(pid)), source)
    } else {
        let path = if gs(&e, "installPath").is_empty() { None } else { Some(PathBuf::from(gs(&e, "installPath"))) };
        let pj = path.as_ref().map(|p| read_obj(&p.join(".claude-plugin").join("plugin.json"))).unwrap_or_default();
        let user_scope = entries.iter().any(|x| gs(x, "scope") == "user");
        let enabled = if user_scope { Value::Bool(ClaudeCode::enabled_map().get(pid) == Some(&Value::Bool(true))) } else { Value::Null };
        let source = describe_claude_source(ClaudeCode::marketplaces().get(mkt));
        (path, pj, !entries.is_empty(), e.get("version").cloned().unwrap_or(Value::Null), enabled, source)
    };
    let mut app = Map::new();
    app.insert("app".into(), Value::from("claude"));
    app.insert("id".into(), Value::from(pid));
    app.insert("installed".into(), Value::Bool(installed));
    app.insert("version".into(), version);
    app.insert("enabled".into(), enabled);
    app.insert("source".into(), Value::from(source));
    app.insert("description".into(), Value::from(gs(&Value::Object(pj.clone()), "description")));
    app.insert("path".into(), path.map(|p| Value::from(display(&p))).unwrap_or(Value::Null));
    app.insert("projects".into(), Value::Array(project_rows(&projects)));
    merge_into(app, status)
}

/// 一个 Codex 插件在本机的情况，以及用过它的项目。folder_id 是它在 ~/.yuwanplugins 里的文件夹名。
pub fn codex_app(codex: &Codex, cid: &str, usage: &Usage, status: Option<&Value>, folder_id: &str) -> Value {
    let (name, mkt) = split_id(cid);
    let conf = Codex::plugin_conf(cid);
    let versions = codex.cache_versions(name, Some(mkt));
    let pj = versions.last().map(|v| read_obj(&v.join(CODEX_MANIFEST))).unwrap_or_default();
    let yuwan = format!("yuwan:{folder_id}");
    let projects = merge_projects(&[usage.get("codex", cid), if folder_id.is_empty() { None } else { usage.get("codex", &yuwan) }]);
    let mut app = Map::new();
    app.insert("app".into(), Value::from("codex"));
    app.insert("id".into(), Value::from(cid));
    app.insert("installed".into(), Value::Bool(conf.is_some()));
    app.insert("version".into(), versions.last().map(|p| Value::from(file_name(p))).unwrap_or(Value::Null));
    app.insert("enabled".into(), Value::Bool(conf.as_ref().map(Codex::conf_enabled).unwrap_or(false)));
    app.insert("source".into(), Value::from(format!("插件源 {mkt}")));
    app.insert("description".into(), Value::from(gs(&Value::Object(pj.clone()), "description")));
    app.insert("path".into(), versions.last().map(|p| Value::from(display(p))).unwrap_or(Value::Null));
    app.insert("projects".into(), Value::Array(project_rows(&projects)));
    merge_into(app, status)
}

/// 未受管的插件如果是从 git 仓库装的，返回仓库地址，方便一键接管
pub fn adoptable_repo(pid: &str) -> String {
    let src = ClaudeCode::marketplaces().get(split_id(pid).1).and_then(|m| m.get("source")).cloned().unwrap_or(Value::Null);
    match gs(&src, "source") {
        "github" if !gs(&src, "repo").is_empty() => format!("https://github.com/{}.git", gs(&src, "repo")),
        "git" => gs(&src, "url").to_string(),
        _ => String::new(),
    }
}

// 移植：只装在一边的插件，把它的 skill 复制一份到 ~/.yuwanplugins/<名字>，补上另一边的清单再链接过去。
// skill 是两边通用的格式；钩子、MCP 服务、App 集成两边格式不同，不带过去。

fn has_skill(d: &Path) -> bool {
    fs::read_dir(d).map(|rd| rd.flatten().any(|e| e.path().join("SKILL.md").is_file())).unwrap_or(false)
}

/// 插件里 skill 所在的文件夹（相对插件根目录，如 ./skills/），没有 skill 返回 None
pub fn skills_rel(folder: &Path, app: &str) -> Option<String> {
    let man = read_obj(&folder.join(if app == "codex" { CODEX_MANIFEST } else { ".claude-plugin/plugin.json" }));
    let mut rels: Vec<String> = Vec::new();
    if let Some(Value::String(s)) = man.get("skills") {
        rels.push(s.clone());
    }
    rels.push("./skills/".into());
    for rel in rels {
        let name = rel.strip_prefix("./").unwrap_or(&rel).trim_matches('/').to_string();
        let d = folder.join(&name);
        if !name.is_empty() && !name.contains("..") && d.is_dir() && has_skill(&d) {
            return Some(format!("./{name}/"));
        }
    }
    None
}

/// 插件里每个 skill 的名字：SKILL.md 开头写的 name，没写就用文件夹名
pub fn skill_names(folder: &Path, app: &str) -> Vec<String> {
    let Some(rel) = skills_rel(folder, app) else { return Vec::new() };
    let dir = folder.join(rel.trim_start_matches("./").trim_end_matches('/'));
    let mut dirs: Vec<PathBuf> = fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join("SKILL.md").is_file()).collect()).unwrap_or_default();
    dirs.sort();
    let fm = Regex::new(r"(?ms)^---\s*$(.*?)^---\s*$").unwrap();
    let name_re = Regex::new(r#"(?m)^name:\s*["']?(.+?)["']?\s*$"#).unwrap();
    dirs.iter()
        .map(|d| {
            let text = fs::read(d.join("SKILL.md")).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
            let head: String = text.chars().take(3000).collect();
            fm.captures(&head)
                .and_then(|m| name_re.captures(&m[1]).map(|n| n[1].trim().to_string()))
                .unwrap_or_else(|| file_name(d))
        })
        .collect()
}

/// 这个 app 里装的是不是移植过来的副本；是的话返回它的来历
pub fn port_info(a: &Value) -> Option<Value> {
    let path = gs(a, "path");
    if path.is_empty() {
        return None;
    }
    read_json(&Path::new(path).join(PORT_MARK)).filter(Value::is_object)
}

/// 移植到 to_app 时从哪复制：另一个 app 里装好的那份（不能是移植来的），要有 skill
pub fn port_source(r: &Value, to_app: &str) -> Option<(Value, PathBuf, String)> {
    for a in ga(r, "apps") {
        let has_port = a.get("port").map(|p| !p.is_null()).unwrap_or(false);
        if gb(a, "installed") && gs(a, "app") != to_app && !has_port && !gs(a, "path").is_empty() {
            let folder = PathBuf::from(gs(a, "path"));
            if let Some(rel) = skills_rel(&folder, gs(a, "app")) {
                return Some((a.clone(), folder, rel));
            }
        }
    }
    None
}

/// 每个还没装的 app 能怎么装：native 直接装，adopt 先交给插件中心管理再装，port 移植 skill；装不了的写原因
pub fn install_options(r: &Value) -> Map<String, Value> {
    let installed: Vec<&str> = ga(r, "apps").iter().filter(|a| gb(a, "installed")).map(|a| gs(a, "app")).collect();
    let supports: Vec<&str> = ga(r, "supports").iter().filter_map(Value::as_str).collect();
    let managed = r.get("managed").map(|m| !m.is_null()).unwrap_or(false);
    let mut out = Map::new();
    for x in APPS {
        if installed.contains(&x) {
            continue;
        }
        let v = if supports.contains(&x) && (managed || !gs(r, "repo").is_empty()) {
            json!({"how": if managed { "native" } else { "adopt" }})
        } else if managed {
            json!({"how": null, "why": format!("仓库里没有 {} 的插件清单", app_name(x))})
        } else if let Some((src, _, _)) = port_source(r, x) {
            json!({"how": "port", "from": gs(&src, "app")})
        } else if installed.is_empty() {
            json!({"how": null, "why": "哪边都没装，找不到插件的文件"})
        } else {
            let others: Vec<&str> = installed.iter().map(|y| app_name(y)).collect();
            json!({"how": null, "why": format!("它没有可以移植的 skill，靠 {} 专用的 MCP 服务或钩子工作", others.join("、"))})
        };
        out.insert(x.to_string(), v);
    }
    out
}

fn version_text(a: &Value) -> String {
    match a.get("version") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// 把插件移植到 to_app；已经移植过就用源插件的最新内容重新复制一遍
pub fn port_plugin(r: &Value, to_app: &str) -> R<Vec<String>> {
    let Some((a, folder, rel)) = port_source(r, to_app) else {
        bail!("{} 没有可以移植到 {} 的 skill。", gs(r, "name"), app_name(to_app));
    };
    let name = Regex::new(r"[^\w.-]").unwrap().replace_all(split_id(gs(&a, "id")).0, "-").to_string();
    let dest = PLUGINS_DIR.join(&name);
    if dest.exists() && !dest.join(PORT_MARK).is_file() {
        bail!("{} 已经存在，而且不是插件中心移植出来的，没有动它。", display(&dest));
    }
    fs::create_dir_all(&*TMP_DIR)?;
    let stage = TMP_DIR.join(format!("port-{name}-{}", std::process::id()));
    rmtree(&stage)?;
    copy_dir(&folder, &stage, PORT_SKIP)?;
    let version = if version_text(&a).is_empty() { "0.0.0".to_string() } else { version_text(&a) };
    let desc = if gs(&a, "description").is_empty() { gs(r, "description") } else { gs(&a, "description") };
    let mut claude_man = json!({"name": name, "version": version, "description": desc});
    if rel != "./skills/" {
        claude_man["skills"] = Value::from(rel.clone());
    }
    write_json(&stage.join(".claude-plugin").join("plugin.json"), &claude_man)?;
    write_json(&stage.join(CODEX_MANIFEST), &json!({"name": name, "version": version, "description": desc, "skills": rel}))?;
    write_json(
        &stage.join(PORT_MARK),
        &json!({"from": gs(&a, "app"), "source_id": gs(&a, "id"), "source_version": version, "folder": display(&dest), "at": stamp()}),
    )?;
    // 副本是生成的，直接换掉；两边的链接指向的是路径，换完照样有效
    rmtree(&dest)?;
    fs::create_dir_all(&*PLUGINS_DIR)?;
    move_path(&stage, &dest)?;
    let mut notes = vec![format!("从 {} 复制 skill 到 {}", app_name(gs(&a, "app")), display(&dest))];
    notes.extend(if to_app == "claude" { ClaudeCode::new().link(&name, &dest)? } else { Codex::new().link(&name, &dest, false)? });
    Ok(notes)
}

/// 移植出来的副本两边都不用了，挪进备份
pub fn cleanup_port(folder: &Path) -> R<Vec<String>> {
    if !folder.join(PORT_MARK).is_file() {
        return Ok(Vec::new());
    }
    let name = file_name(folder);
    if links_to(&CLAUDE_SKILLS.join(&name), folder) || same_path(Codex::new().entry_path(&name).as_deref(), Some(folder)) {
        return Ok(Vec::new());
    }
    Ok(vec![format!("移植的副本挪到 {}", display(&backup(folder, &name)?))])
}

/// 源插件更新过的移植副本，重新复制一遍
pub fn refresh_ports() -> Vec<String> {
    let mut notes = Vec::new();
    let Ok(state) = build_state() else { return notes };
    for r in ga(&state, "plugins") {
        for a in ga(r, "apps") {
            let stale = a.get("port").map(|p| gb(p, "stale")).unwrap_or(false);
            if gb(a, "installed") && stale {
                match port_plugin(r, gs(a, "app")) {
                    Ok(ns) => notes.extend(ns.into_iter().map(|n| format!("[{}] {}：{n}", gs(r, "key"), app_name(gs(a, "app"))))),
                    Err(e) => notes.push(format!("[{}] 重新移植到 {} 失败：{e}", gs(r, "key"), app_name(gs(a, "app")))),
                }
            }
        }
    }
    for n in &notes {
        log(n);
    }
    notes
}

fn row_index(rows: &mut Vec<Value>, name: &str) -> usize {
    if let Some(i) = rows.iter().position(|r| gs(r, "key") == name) {
        return i;
    }
    rows.push(json!({"key": name, "name": name, "version": "", "description": "", "managed": null, "repo": "", "apps": [], "supports": []}));
    rows.len() - 1
}

/// 本机所有插件，同名的合成一行；受管的带上来源信息
pub fn machine_plugins(managed: &[Value], claude: &ClaudeCode, codex: &Codex) -> Vec<Value> {
    let usage = scan_usage();
    let mut rows: Vec<Value> = Vec::new();
    let mut owned: HashSet<(String, String)> = HashSet::new();
    for v in managed {
        let man = v.get("manifest").cloned().unwrap_or(Value::Null);
        let mut apps: Vec<Value> = ga(v, "claude").iter().map(|t| claude_app(claude, gs(t, "id"), &usage, Some(t))).collect();
        if let Some(c) = v.get("codex").filter(|c| c.is_object()) {
            apps.push(codex_app(codex, gs(c, "id"), &usage, Some(c), gs(v, "id")));
        }
        owned.extend(apps.iter().map(|a| (gs(a, "app").to_string(), gs(a, "id").to_string())));
        let mut supports = Vec::new();
        if !ga(&man, "claude_plugins").is_empty() {
            supports.push("claude");
        }
        if man.get("codex_name").map(|x| !x.is_null()).unwrap_or(false) {
            supports.push("codex");
        }
        let title = if gs(&man, "title").is_empty() { gs(v, "id") } else { gs(&man, "title") };
        rows.push(json!({"key": gs(v, "id"), "name": title, "version": gs(&man, "version"), "description": gs(&man, "description"),
                         "managed": v, "repo": gs(v, "repo"), "apps": apps, "supports": supports}));
    }
    let mut ids: BTreeSet<String> = ClaudeCode::installed().keys().cloned().collect();
    ids.extend(ClaudeCode::skills_dir().keys().cloned());
    for pid in ids {
        if owned.contains(&("claude".to_string(), pid.clone())) {
            continue;
        }
        let i = row_index(&mut rows, split_id(&pid).0);
        let a = claude_app(claude, &pid, &usage, None);
        let path = gs(&a, "path").to_string();
        let r = rows[i].as_object_mut().expect("object");
        if gs(&Value::Object(r.clone()), "repo").is_empty() {
            r.insert("repo".into(), Value::from(adoptable_repo(&pid)));
        }
        if !gs(&Value::Object(r.clone()), "repo").is_empty() && !path.is_empty() {
            // 从 git 装的，看装好的副本里带了哪些 app 的清单
            let mut supports = vec!["claude"];
            if Path::new(&path).join(CODEX_MANIFEST).is_file() {
                supports.push("codex");
            }
            r.insert("supports".into(), json!(supports));
        }
        r.get_mut("apps").and_then(Value::as_array_mut).expect("array").push(a);
    }
    for cid in Codex::configured() {
        if owned.contains(&("codex".to_string(), cid.clone())) {
            continue;
        }
        let i = row_index(&mut rows, split_id(&cid).0);
        let a = codex_app(codex, &cid, &usage, None, "");
        rows[i]["apps"].as_array_mut().expect("array").push(a);
    }
    let official_re = Regex::new(r"^(openai-|claude-plugins-official$)").unwrap();
    for r in rows.iter_mut() {
        if gs(r, "description").is_empty() {
            let d = ga(r, "apps").iter().map(|a| gs(a, "description").to_string()).find(|d| !d.is_empty()).unwrap_or_default();
            r["description"] = Value::from(d);
        }
        let link = if let Some(m) = r.get("managed").filter(|m| m.is_object()) {
            gs(m, "web").to_string()
        } else if !gs(r, "repo").is_empty() {
            repo_web_url(gs(r, "repo"))
        } else {
            String::new()
        };
        r["link"] = Value::from(link);
        // 两边自带的官方插件由 app 自己管，页面上不给删除和同步
        let apps = r["apps"].as_array_mut().expect("array");
        for a in apps.iter_mut() {
            let official = official_re.is_match(split_id(gs(a, "id")).1);
            a["official"] = Value::Bool(official);
            a["port"] = port_info(a).unwrap_or(Value::Null);
        }
        // 移植的副本：源插件版本变了就算过期，同步时重新复制
        let snapshot = apps.clone();
        for a in apps.iter_mut() {
            if a["port"].is_null() {
                continue;
            }
            let from = gs(&a["port"], "from").to_string();
            let src = snapshot.iter().find(|x| gs(x, "app") == from && gb(x, "installed") && x["port"].is_null());
            let src_version = src.map(version_text).unwrap_or_else(|| "None".into());
            a["port"]["source_missing"] = Value::Bool(src.is_none());
            a["port"]["stale"] = Value::Bool(src.is_some() && src_version != gs(&a["port"], "source_version"));
        }
        let all_official = !apps.is_empty() && apps.iter().all(|a| gb(a, "official"));
        r["official"] = Value::Bool(all_official);
        let install = install_options(r);
        let installable: Vec<String> = install.iter().filter(|(_, o)| o.get("how").map(|h| !h.is_null()).unwrap_or(false)).map(|(k, _)| k.clone()).collect();
        r["install"] = Value::Object(install);
        r["installable"] = json!(installable);
    }
    rows.sort_by_key(|r| (r.get("managed").map(Value::is_null).unwrap_or(true), gs(r, "name").to_lowercase()));
    rows
}

pub fn build_state() -> R<Value> {
    let cfg = load_config();
    let mut st = load_state();
    let (claude, codex) = (ClaudeCode::new(), Codex::new());
    let views: Vec<Value> = cfg.plugins.iter().map(|p| plugin_view(p, &mut st, &claude, &codex)).collect();
    save_state(&st)?;
    let mut plugins = machine_plugins(&views, &claude, &codex);
    let checks = st.get("verify").cloned().unwrap_or(Value::Null);
    let repairs = st.get("repairs").cloned().unwrap_or(Value::Null);
    for r in plugins.iter_mut() {
        // 上次真实检查的结果，以及后台检查最近一次修复
        if let Some(apps) = r["apps"].as_array_mut() {
            for a in apps.iter_mut() {
                let key = format!("{}:{}", gs(a, "app"), gs(a, "id"));
                let rkey = format!("{}:{}", gs(a, "app"), split_id(gs(a, "id")).0);
                a["verify"] = checks.get(&key).cloned().unwrap_or(Value::Null);
                a["repair"] = repairs.get(&rkey).cloned().unwrap_or(Value::Null);
            }
        }
    }
    Ok(json!({
        "generated_at": stamp(),
        "hub": {
            "version": HUB_VERSION, "home": display(&HUB_DIR), "plugins_dir": display(&PLUGINS_DIR),
            "auto": auto_status(&cfg), "last_auto": st.get("last_auto").cloned().unwrap_or(Value::Null),
            "guard": {"enabled": cfg.guard.enabled, "last": st.get("last_guard").cloned().unwrap_or(Value::Null)},
            "launcher": launcher_line(&["auto"]),
        },
        "tools": {
            "claude": claude.exe.as_deref().map(display).unwrap_or_default(),
            "codex": codex.exe.as_deref().map(display).unwrap_or_default(),
            "git": which_git().as_deref().map(display).unwrap_or_default(),
        },
        "plugins": plugins,
        "log": tail_log(80),
    }))
}
