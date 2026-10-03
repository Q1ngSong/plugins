//! 操作：检查更新、更新、添加、安装、卸载、同步、真实检查、后台检查、锁定、另存，以及接口分发
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::bail;
use crate::claude::{split_id, ClaudeCode};
use crate::codex::Codex;
use crate::gitx::*;
use crate::schedule::*;
use crate::store::*;
use crate::platform;
use crate::util::*;
use crate::view::*;

/// 同一时间只允许一个操作（进程内）；跨进程用 hub.lock
static OP: Mutex<()> = Mutex::new(());

/// 检查更新：拉取远端看有没有新版本（锁定的跳过），再做一遍真实检查。不动任何安装。
pub fn op_check() -> R<Vec<String>> {
    let cfg = load_config();
    let mut st = load_state();
    let mut notes = Vec::new();
    for p in &cfg.plugins {
        if p.locked {
            continue;
        }
        let result = fetch(p);
        let m = memo(&mut st, &p.id);
        set(m, "checked_at", stamp());
        let d = match result {
            Ok(d) => {
                set(m, "error", Value::Null);
                d
            }
            Err(e) => {
                set(m, "error", e.to_string());
                notes.push(format!("[{}] 检查失败：{e}", p.id));
                continue;
            }
        };
        let remote = commit_info(&d, &format!("origin/{}", p.branch));
        let head = commit_info(&d, "HEAD");
        if let (Some(r), Some(h)) = (remote, head) {
            if r.sha != h.sha {
                notes.push(format!("[{}] 有新版本 {}：{}", p.id, r.short, r.subject));
            }
        }
    }
    save_state(&st)?;
    for n in &notes {
        log(n);
    }
    notes.extend(verify_ours()?);
    Ok(notes)
}

fn update_one(p: &PluginCfg, m: &mut Map<String, Value>, claude: &ClaudeCode, codex: &Codex, force: bool) -> R<Vec<String>> {
    let tag = format!("[{}]", p.id);
    let mut notes = Vec::new();
    let d = repo_dir(p);
    let mut pending: Option<String> = None;
    let synced = m.get("commit").and_then(Value::as_str).map(str::to_string);
    let (old, new) = if p.locked {
        // 锁定的不拉取，只检查两边的链接
        let h = head_sha(&d);
        (h.clone(), h)
    } else {
        match sync_clone(p, synced.as_deref()) {
            Ok((old, new)) => (old, Some(new)),
            Err(HubError::LocalChanges(msg)) => {
                // 有修改就不拉取，但照样检查两边的链接
                notes.push(format!("{tag} {msg}"));
                pending = Some(msg);
                let h = head_sha(&d);
                (h.clone(), h)
            }
            Err(e) => return Err(e),
        }
    };
    if old != new {
        // 两边都链接到这个文件夹，拉取完就已经用上新文件了
        let info = commit_info(&d, "HEAD");
        let short = |s: &Option<String>| s.as_deref().map(|x| x.chars().take(7).collect::<String>()).unwrap_or_else(|| "无".into());
        notes.push(format!("{tag} {} {} → {}：{}", display(&d), short(&old), short(&new), info.map(|c| c.subject).unwrap_or_default()));
    }
    let man = read_manifests(&d, None);
    if p.target("claude") {
        for c in &man.claude_plugins {
            notes.extend(claude.link(&c.name, &d.join(&c.path))?.into_iter().map(|n| format!("{tag} Claude Code：{n}")));
        }
    }
    if p.target("codex") {
        if let Some(name) = &man.codex_name {
            notes.extend(codex.link(name, &d, force)?.into_iter().map(|n| format!("{tag} Codex：{n}")));
        }
    }
    set(m, "updated_at", stamp());
    set(m, "checked_at", stamp());
    set(m, "error", pending.clone().map(Value::from).unwrap_or(Value::Null));
    if pending.is_none() && !p.locked {
        // 只记真正从远端拉到的提交，本地提交不能算进去
        set(m, "commit", new.map(Value::from).unwrap_or(Value::Null));
    }
    for n in &notes {
        log(n);
    }
    Ok(notes)
}

pub fn op_update(pid: Option<&str>, force: bool) -> R<Vec<String>> {
    let cfg = load_config();
    let mut st = load_state();
    let (claude, codex) = (ClaudeCode::new(), Codex::new());
    let mut notes = Vec::new();
    for p in &cfg.plugins {
        if pid.is_some_and(|x| x != p.id) {
            continue;
        }
        let m = memo(&mut st, &p.id);
        match update_one(p, m, &claude, &codex, force) {
            Ok(ns) => notes.extend(ns),
            Err(e) => {
                set(m, "error", e.to_string());
                set(m, "checked_at", stamp());
                notes.push(format!("[{}] 更新失败：{e}", p.id));
                log(notes.last().expect("pushed"));
            }
        }
        save_state(&st)?;
    }
    Ok(notes)
}

pub struct Target {
    pub key: String,
    pub app: &'static str,
    pub name: String,
    pub folder: PathBuf,
}

/// 插件中心装进两边的每一份：受管插件（按它要装的 app）和移植的副本
pub fn linked_targets(cfg: &Config) -> Vec<Target> {
    let mut out = Vec::new();
    for p in &cfg.plugins {
        let d = PLUGINS_DIR.join(&p.id);
        if !d.join(".git").exists() {
            continue;
        }
        let man = read_manifests(&d, None);
        if p.target("claude") {
            out.extend(man.claude_plugins.iter().map(|c| Target { key: p.id.clone(), app: "claude", name: c.name.clone(), folder: d.join(&c.path) }));
        }
        if p.target("codex") {
            if let Some(name) = &man.codex_name {
                out.push(Target { key: p.id.clone(), app: "codex", name: name.clone(), folder: d.clone() });
            }
        }
    }
    // 移植的副本装在来源的另一个 app 里
    let mut marks: Vec<PathBuf> = fs::read_dir(&*PLUGINS_DIR).map(|rd| rd.flatten().map(|e| e.path().join(PORT_MARK)).filter(|m| m.is_file()).collect()).unwrap_or_default();
    marks.sort();
    for mark in marks {
        let src = read_obj(&mark).get("from").and_then(Value::as_str).unwrap_or("").to_string();
        if APPS.contains(&src.as_str()) {
            let folder = mark.parent().expect("parent").to_path_buf();
            let name = file_name(&folder);
            out.push(Target { key: name.clone(), app: if src == "codex" { "claude" } else { "codex" }, name, folder });
        }
    }
    out
}

/// 后台检查：两边的插件配置有没有被别的程序改掉（比如切换服务商的工具重写了配置文件），改掉了就修复，
/// 然后做一遍真实检查。
pub fn op_guard() -> R<Vec<String>> {
    let cfg = load_config();
    let mut st = load_state();
    let (claude, codex) = (ClaudeCode::new(), Codex::new());
    let mut notes = Vec::new();
    for t in linked_targets(&cfg) {
        let s = if t.app == "claude" { claude.status(&t.name, &t.folder, None, None) } else { codex.status(&t.name, &t.folder, None, None) };
        if gs(&s, "state") == "ok" {
            continue;
        }
        let mut problems: Vec<String> = ga(&s, "problems").iter().filter_map(Value::as_str).map(str::to_string).collect();
        if problems.is_empty() {
            problems.push(gs(&s, "state").to_string());
        }
        let fixed = if t.app == "claude" { claude.link(&t.name, &t.folder).map(|_| ()) } else { codex.link(&t.name, &t.folder, false).map(|_| ()) };
        let ok = match fixed {
            Ok(()) => true,
            Err(e) => {
                problems.push(format!("修复失败：{e}"));
                false
            }
        };
        let line = format!("[{}] {}：发现{}，{}", t.key, app_name(t.app), problems.join("；"), if ok { "已修复" } else { "没修好" });
        log(&line);
        notes.push(line);
        sub(&mut st, "repairs").insert(format!("{}:{}", t.app, t.name), json!({"at": stamp(), "what": problems, "ok": ok}));
    }
    // 插件中心管的独立 skill：两边的链接还在不在
    for (app, name, problems, ok) in crate::skills::guard() {
        let line = format!("[{name}] {}：发现{}，{}", app_name(&app), problems.join("；"), if ok { "已修复" } else { "没修好" });
        log(&line);
        notes.push(line);
        sub(&mut st, "repairs").insert(format!("{app}:skill:{name}"), json!({"at": stamp(), "what": problems, "ok": ok}));
    }
    st.insert("last_guard".into(), Value::from(stamp()));
    save_state(&st)?;
    notes.extend(verify_ours()?);
    Ok(notes)
}

/// 后台任务每次要做的事：后台检查到了一天就检查并修复配置；自动更新到点了就更新
pub fn tick() -> R<Vec<String>> {
    let cfg = load_config();
    let st = load_state();
    let mut notes = Vec::new();
    if cfg.guard.enabled && due(st.get("last_guard").and_then(Value::as_str), GUARD_MINUTES) {
        notes.extend(op_guard()?);
    }
    let last_auto = load_state().get("last_auto").and_then(|x| x.get("at")).and_then(Value::as_str).map(str::to_string);
    if cfg.auto_update.enabled && due(last_auto.as_deref(), auto_minutes(&cfg)) {
        let mut done = op_update(None, false)?;
        done.extend(refresh_ports());
        let mut st = load_state();
        let tail: Vec<String> = done.iter().rev().take(10).rev().cloned().collect();
        st.insert("last_auto".into(), json!({"at": stamp(), "notes": tail}));
        save_state(&st)?;
        notes.extend(done);
    }
    Ok(notes)
}

pub fn validate_branch(branch: &str) -> R<String> {
    let branch = branch.trim().to_string();
    let re = Regex::new(r"^[\w][\w./-]*$").unwrap();
    if !re.is_match(&branch) || branch.contains("..") {
        bail!("分支名不合法：{branch:?}");
    }
    Ok(branch)
}

pub fn normalize_repo(repo: &str) -> R<String> {
    let mut repo = repo.trim().to_string();
    if Regex::new(r"^[\w.-]+/[\w.-]+$").unwrap().is_match(&repo) {
        // owner/repo 简写
        repo = format!("https://github.com/{repo}.git");
    }
    if !Regex::new(r"^(https://|http://|git@|ssh://|file://)").unwrap().is_match(&repo) {
        bail!("仓库地址要以 https:// 或 git@ 开头，或者写成 owner/repo。");
    }
    Ok(repo)
}

pub fn repo_id(repo: &str) -> String {
    let trimmed = repo.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let last = last.rsplit(':').next().unwrap_or(last);
    Regex::new(r"[^\w.-]").unwrap().replace_all(last, "-").to_string()
}

/// 添加前先看看仓库：有哪些分支，每个分支的最新提交和插件版本号
pub fn probe_repo(repo: &str) -> R<Value> {
    let repo = normalize_repo(repo)?;
    fs::create_dir_all(&*TMP_DIR)?;
    let d = TMP_DIR.join(format!("probe-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    let result = (|| -> R<Vec<Value>> {
        // 每个分支只取最新一个提交，大文件先不下载；插件清单很小，都在里面
        git_t(&["clone", "--bare", "--depth=1", "--no-single-branch", "--filter=blob:limit=256k", &repo, &display(&d)], None, 300, true)?;
        let default = git_soft(&["symbolic-ref", "--short", "HEAD"], Some(&d)).trim().to_string();
        let fmt = "--format=%(refname:short)%1f%(objectname)%1f%(committerdate:iso-strict)%1f%(contents:subject)";
        let mut branches = Vec::new();
        for line in git(&["for-each-ref", fmt, "refs/heads"], Some(&d))?.lines() {
            let mut parts: Vec<&str> = line.split('\x1f').collect();
            parts.resize(4, "");
            let (name, sha, date, subject) = (parts[0], parts[1], parts[2], parts[3]);
            if name.is_empty() {
                continue;
            }
            let man = read_manifests(&d, Some(name));
            branches.push(json!({"name": name, "sha": sha, "date": date, "subject": subject, "default": name == default,
                                 "version": man.version, "title": man.title, "description": man.description,
                                 "claude": !man.claude_plugins.is_empty(), "codex": man.codex_name.is_some()}));
        }
        Ok(branches)
    })();
    let _ = rmtree(&d);
    let mut branches = result?;
    branches.sort_by(|a, b| gs(b, "date").cmp(gs(a, "date")));
    branches.sort_by_key(|b| !gb(b, "default")); // 默认分支放最前，其余按最近提交排
    let pid = repo_id(&repo);
    let managed = load_config().plugins.iter().any(|p| p.id == pid);
    Ok(json!({"repo": repo, "id": pid, "branches": branches, "managed": managed}))
}

/// 接管一个插件仓库，装到 apps 里列出的 app
pub fn add_plugin(repo: &str, branch: &str, apply: bool, apps: &[String]) -> R<Vec<String>> {
    let repo = normalize_repo(repo)?;
    let pid = repo_id(&repo);
    let mut cfg = load_config();
    if cfg.plugins.iter().any(|p| p.id == pid) {
        bail!("{pid} 已经在管理列表里了。");
    }
    if apps.is_empty() {
        bail!("至少选一个要安装的 app。");
    }
    let branch = validate_branch(&if branch.trim().is_empty() { default_branch(&repo)? } else { branch.to_string() })?;
    let mut p = PluginCfg { id: pid.clone(), repo: repo.clone(), branch: branch.clone(), targets: None, locked: false, extra: Map::new() };
    for x in APPS {
        p.set_target(x, apps.iter().any(|a| a == x));
    }
    let d = ensure_clone(&p)?;
    let man = read_manifests(&d, None);
    if man.claude_plugins.is_empty() && man.codex_name.is_none() {
        rmtree(&d)?;
        bail!("这个仓库里既没有 .claude-plugin/marketplace.json，也没有 .codex-plugin/plugin.json，不像插件仓库。");
    }
    cfg.plugins.push(p);
    remember_proxy(&mut cfg);
    save_config(&cfg)?;
    let mut notes = vec![format!("[{pid}] 开始管理（{branch} 分支）")];
    log(&notes[0]);
    if apply {
        notes.extend(op_update(Some(&pid), false)?);
        let picked: Vec<String> = apps.iter().filter(|a| APPS.contains(&a.as_str())).cloned().collect();
        notes.extend(verify_plugin(&pid, Some(&picked), true)?);
    }
    Ok(notes)
}

/// 后台检查：每天看一次两边的插件配置有没有被别的程序改掉，改掉了就修复
pub fn set_guard(enabled: bool) -> R<Vec<String>> {
    let mut cfg = load_config();
    remember_proxy(&mut cfg);
    cfg.guard.enabled = enabled;
    let mut notes = vec![format!("已{}后台检查", if enabled { "开启" } else { "关闭" }), apply_schedule(&mut cfg)?];
    for n in &notes {
        log(n);
    }
    if enabled {
        // 马上先查一遍，不用等第一次后台任务（它自己会写日志）
        notes.extend(op_guard()?);
    }
    Ok(notes)
}

pub fn find_row(key: &str) -> R<Value> {
    let state = build_state()?;
    ga(&state, "plugins").iter().find(|r| gs(r, "key") == key).cloned().ok_or_else(|| HubError::Msg(format!("本机没有这个插件：{key}")))
}

/// 插件中心装进这个 app 的那份实际放在哪：受管插件是 ~/.yuwanplugins 里的克隆，移植的是副本文件夹，
/// 独立的 skill 是 ~/.yuwanplugins/skills 里统一存放的那份
pub fn ours_root(row: &Value, a: &Value) -> Option<PathBuf> {
    if gs(row, "kind") == "skill" {
        return row["skill"].get("folder").and_then(Value::as_str).filter(|_| a.get("state").is_some()).map(PathBuf::from);
    }
    if let Some(port) = a.get("port").filter(|p| p.is_object()) {
        if !gs(port, "folder").is_empty() {
            return Some(PathBuf::from(gs(port, "folder")));
        }
    }
    let managed = row.get("managed").map(|m| !m.is_null()).unwrap_or(false);
    if managed && a.get("state").is_some() && !gs(a, "folder").is_empty() {
        // 带 state 的是插件中心装的那一份
        return Some(PathBuf::from(gs(a, "folder")));
    }
    None
}

/// 真实检查：让装了插件的每个 app 自己确认看得到它的每个 skill，结果记在 state.json。
///
/// 插件中心装的那份还要确认 app 读的正是 ~/.yuwanplugins 里的文件。渲染过 Codex 的模型提示，就顺带看它的技能清单超没超上限，
/// 结果记在 state.json 的 codex_skills，首页据此提醒。返回 (通过的, 没通过的, 技能清单超了上限时的说明)。
pub fn run_verify(rows: &[Value], apps: Option<&[String]>, only_ours: bool) -> R<(Vec<String>, Vec<String>, Option<String>)> {
    let mut st = load_state();
    let (mut claude, mut codex) = (ClaudeCode::new(), Codex::new()); // 同一轮里 Codex 的模型提示只渲染一次
    let (mut good, mut bad) = (Vec::new(), Vec::new());
    for row in rows {
        for a in ga(row, "apps") {
            let app = gs(a, "app");
            if !gb(a, "installed") || apps.is_some_and(|xs| !xs.iter().any(|x| x == app)) {
                continue;
            }
            let root = ours_root(row, a);
            if only_ours && root.is_none() {
                continue;
            }
            let result = if gs(row, "kind") == "skill" {
                // 独立的 skill：Codex 看模型提示；Claude Code 没有列出 skill 的命令，只能核对文件
                if app == "claude" { Ok(crate::skills::check_claude(a, root.as_deref())) } else { codex.verify_skill(gs(row, "name"), root.as_deref()) }
            } else {
                let expect = if gs(a, "path").is_empty() { Vec::new() } else { skill_names(Path::new(gs(a, "path")), app) };
                if app == "claude" { claude.verify(gs(a, "id"), &expect, root.as_deref()) } else { codex.verify(gs(a, "id"), &expect, root.as_deref()) }
            };
            let (ok, detail) = result.unwrap_or_else(|e| (false, format!("检查时出错：{e}")));
            sub(&mut st, "verify").insert(format!("{app}:{}", gs(a, "id")), json!({"ok": ok, "detail": detail, "at": stamp()}));
            let line = format!("[{}] {} 真实检查{}：{detail}", gs(row, "key"), app_name(app), if ok { "通过" } else { "没通过" });
            if ok { good.push(line) } else { bad.push(line) }
        }
    }
    let over = if codex.rendered() { record_budget(&mut codex, &mut st) } else { None };
    save_state(&st)?;
    for n in good.iter().chain(bad.iter()) {
        log(n);
    }
    Ok((good, bad, over))
}

/// 看 Codex 的技能清单超没超上限，记进 state 和日志。返回超了时的说明
fn record_budget(codex: &mut Codex, st: &mut State) -> Option<String> {
    let (ok, detail) = codex.budget().unwrap_or_else(|e| (false, format!("检查时出错：{e}")));
    st.insert("codex_skills".into(), json!({"ok": ok, "detail": detail, "at": stamp()}));
    let line = format!("[技能清单] Codex：{detail}");
    log(&line);
    (!ok).then_some(line)
}

/// 只看 Codex 的技能清单：删掉用不上的插件或技能以后，不用等后台检查就能再看一次
fn act_budget() -> R<Vec<String>> {
    let mut st = load_state();
    let mut codex = Codex::new();
    codex.prompt_text()?; // 渲染不出来（比如找不到 codex 的命令行）就直接报错，不记进状态
    let line = record_budget(&mut codex, &mut st);
    save_state(&st)?;
    Ok(vec![line.unwrap_or_else(|| format!("[技能清单] Codex：{}", gs(&st["codex_skills"], "detail")))])
}

/// 对一个插件做真实检查。strict 为真时，有没通过的就报错（装完插件时用）。
pub fn verify_plugin(key: &str, apps: Option<&[String]>, strict: bool) -> R<Vec<String>> {
    let (mut good, bad, _) = run_verify(&[find_row(key)?], apps, false)?;
    if strict && !bad.is_empty() {
        bail!("装好了，但真实检查没通过：\n{}", bad.join("\n"));
    }
    good.extend(bad);
    Ok(good)
}

/// 对插件中心装的每一份做真实检查（检查更新和后台检查时用），只把没通过的列出来；Codex 的技能清单超了上限也列出来
pub fn verify_ours() -> R<Vec<String>> {
    let state = build_state()?;
    let (good, mut bad, over) = run_verify(ga(&state, "plugins"), None, true)?;
    bad.extend(over);
    if !bad.is_empty() {
        return Ok(bad);
    }
    Ok(if good.is_empty() { Vec::new() } else { vec![format!("真实检查：{} 项都通过", good.len())] })
}

fn body_apps(body: &Value) -> Vec<String> {
    ga(body, "apps").iter().filter_map(Value::as_str).filter(|x| APPS.contains(x)).map(str::to_string).collect()
}

/// 把插件装到选中的 app：受管的打开对应目标再同步；从 git 装的先交给插件中心管理；只装在一边的移植 skill
pub fn act_install(body: &Value) -> R<Vec<String>> {
    let row = find_row(gs(body, "key"))?;
    let installable: Vec<&str> = ga(&row, "installable").iter().filter_map(Value::as_str).collect();
    let mut apps: Vec<String> = body_apps(body).into_iter().filter(|x| installable.contains(&x.as_str())).collect();
    if apps.is_empty() {
        bail!("没有可以安装的 app。");
    }
    if gs(&row, "kind") == "skill" {
        // 独立的 skill：统一存放一份，链接进选中的 app，再做真实检查。
        // 收编时原来那个 app 的文件夹也换成了链接，所以每个装了它的 app 都要查
        let mut notes = crate::skills::install(&row, &apps)?;
        notes.extend(verify_plugin(gs(&row, "key"), None, true)?);
        return Ok(notes);
    }
    let how = |x: &str| row["install"].get(x).map(|o| gs(o, "how").to_string()).unwrap_or_default();
    let mut notes = Vec::new();
    for x in apps.iter().filter(|x| how(x) == "port") {
        notes.extend(port_plugin(&row, x)?.into_iter().map(|n| format!("[{}] {}：{n}", gs(&row, "key"), app_name(x))));
    }
    for n in &notes {
        log(n);
    }
    let rest: Vec<String> = apps.iter().filter(|x| how(x) != "port").cloned().collect();
    let key = gs(&row, "key").to_string();
    let managed = row.get("managed").filter(|m| m.is_object());
    if let (false, Some(m)) = (rest.is_empty(), managed) {
        let mut cfg = load_config();
        let p = find_managed(&mut cfg, gs(m, "id"))?;
        for x in &rest {
            p.set_target(x, true);
        }
        let id = p.id.clone();
        save_config(&cfg)?;
        notes.extend(op_update(Some(&id), false)?);
    } else if !rest.is_empty() {
        // 从 git 装的：先交给插件中心管理，它自己会做真实检查
        let mut all: Vec<String> = ga(&row, "apps").iter().filter(|a| gb(a, "installed")).map(|a| gs(a, "app").to_string()).collect();
        all.extend(rest.iter().cloned());
        all.sort();
        all.dedup();
        notes.extend(add_plugin(gs(&row, "repo"), "", true, &all)?);
        apps.retain(|x| !rest.contains(x));
    }
    if !apps.is_empty() {
        notes.extend(verify_plugin(&key, Some(&apps), true)?);
    }
    Ok(notes)
}

/// 指定了 app 就只从那个 app 卸载；没指定就从所有装了它的 app 卸载（官方自带的不动）
pub fn act_uninstall(body: &Value) -> R<Vec<String>> {
    if gs(body, "key").starts_with("skill:") {
        let row = find_row(gs(body, "key"))?;
        return if gs(body, "app").is_empty() { crate::skills::uninstall_all(&row) } else { crate::skills::uninstall_app(&row, gs(body, "app")) };
    }
    if !gs(body, "app").is_empty() {
        return uninstall_app(gs(body, "key"), gs(body, "id"), gs(body, "app"));
    }
    let row = find_row(gs(body, "key"))?;
    let mut notes = Vec::new();
    for a in ga(&row, "apps").iter().filter(|a| gb(a, "installed") && !gb(a, "official")) {
        notes.extend(uninstall_app(gs(&row, "key"), gs(a, "id"), gs(a, "app"))?);
    }
    if let Some(v) = row.get("managed").filter(|m| m.is_object()) {
        let mut cfg = load_config();
        if let Some(i) = cfg.plugins.iter().position(|x| x.id == gs(v, "id")) {
            // 受管但哪边都没装上，也一并不再管理
            let p = cfg.plugins.remove(i);
            save_config(&cfg)?;
            notes.push(format!("[{}] 插件中心不再管理它，插件文件夹挪到 {}", gs(&row, "key"), retire(&p)?));
            log(notes.last().expect("pushed"));
        }
    }
    Ok(notes)
}

/// 不再管理时，插件文件夹挪进备份（里面可能有没提交的改动），不直接删
pub fn retire(p: &PluginCfg) -> R<String> {
    let d = repo_dir(p);
    if d.exists() {
        Ok(display(&backup(&d, &p.id)?))
    } else {
        Ok("（没有文件夹）".into())
    }
}

/// 从一个 app 里卸载插件。受管插件两边都卸掉后，插件中心也不再管它、删掉克隆。
pub fn uninstall_app(key: &str, pid: &str, app: &str) -> R<Vec<String>> {
    let row = find_row(key)?;
    let Some(a) = ga(&row, "apps").iter().find(|x| gs(x, "id") == pid && gs(x, "app") == app && gb(x, "installed")).cloned() else {
        bail!("这个 app 里没装这个插件。");
    };
    let (claude, codex) = (ClaudeCode::new(), Codex::new());
    let port = a.get("port").filter(|p| p.is_object()).cloned();
    if app == "claude" {
        claude.remove(pid)?;
    } else {
        codex.remove(pid)?;
        if port.is_some() {
            // 移植时在个人插件源里登记的条目也去掉
            codex.forget(split_id(pid).0)?;
        }
    }
    let mut notes = vec![format!("{}：卸载了 {pid}", app_name(app))];
    if let Some(folder) = port.as_ref().map(|p| gs(p, "folder").to_string()).filter(|f| !f.is_empty()) {
        notes.extend(cleanup_port(Path::new(&folder))?);
    }
    if let Some(v) = row.get("managed").filter(|m| m.is_object()) {
        if a.get("state").is_some() {
            // 带 state 的是插件中心装的那一份
            let mut cfg = load_config();
            let p = find_managed(&mut cfg, gs(v, "id"))?;
            let man = v.get("manifest").cloned().unwrap_or(Value::Null);
            let rest: Vec<&Value> = ga(&row, "apps")
                .iter()
                .filter(|x| !(gs(x, "id") == pid && gs(x, "app") == app) && gb(x, "installed") && x.get("state").is_some())
                .collect();
            if app == "claude" && !rest.iter().any(|x| gs(x, "app") == "claude") {
                p.set_target("claude", false);
            }
            if app == "codex" {
                if let Some(name) = man.get("codex_name").and_then(Value::as_str) {
                    codex.forget(name)?;
                }
                p.set_target("codex", false);
            }
            if rest.is_empty() {
                let removed = p.clone();
                cfg.plugins.retain(|x| x.id != removed.id);
                notes.push(format!("两边都卸掉了，插件中心不再管理它，插件文件夹挪到 {}", retire(&removed)?));
            }
            save_config(&cfg)?;
        }
    }
    let notes: Vec<String> = notes.into_iter().map(|n| format!("[{key}] {n}")).collect();
    for n in &notes {
        log(n);
    }
    Ok(notes)
}

/// 受管的拉取最新并同步到两边；移植的副本从源插件重新复制；其余的让各自的 app 从原插件源更新
pub fn act_sync(body: &Value) -> R<Vec<String>> {
    let row = find_row(gs(body, "key"))?;
    if gs(&row, "kind") == "skill" {
        // 独立的 skill 没有远端可拉：把该有的链接补上，再做一遍真实检查
        let mut notes = crate::skills::sync(&row)?;
        notes.extend(verify_plugin(gs(&row, "key"), None, false)?);
        return Ok(notes);
    }
    if let Some(m) = row.get("managed").filter(|m| m.is_object()) {
        if gb(m, "locked") {
            bail!("{} 已锁定，不同步。先解锁。", gs(&row, "name"));
        }
        // Codex 真实检查没通过时，顺便强制重装一次（重新复制清单、重建缓存里的链接）
        let failed = ga(&row, "apps").iter().any(|a| gs(a, "app") == "codex" && a.get("verify").map(|v| v.get("ok") == Some(&Value::Bool(false))).unwrap_or(false));
        return op_update(Some(gs(m, "id")), failed);
    }
    let (claude, codex) = (ClaudeCode::new(), Codex::new());
    let mut notes = Vec::new();
    for a in ga(&row, "apps") {
        if !gb(a, "installed") || gb(a, "official") {
            continue; // 官方自带的由 app 自己更新
        }
        let app = gs(a, "app");
        if a.get("port").map(|p| p.is_object()).unwrap_or(false) {
            notes.extend(port_plugin(&row, app)?.into_iter().map(|n| format!("{}：{n}", app_name(app))));
            continue;
        }
        let (_, mkt) = split_id(gs(a, "id"));
        if mkt == "skills-dir" {
            continue; // 就地加载的，改了文件就生效，没什么可同步
        }
        if app == "claude" {
            claude.cli(&["plugin", "marketplace", "update", mkt])?;
            let out = claude.cli(&["plugin", "update", gs(a, "id")])?;
            notes.push(format!("Claude Code：{}", out.lines().last().unwrap_or("已更新")));
        } else {
            let _ = codex.cli(&["plugin", "marketplace", "upgrade", mkt]); // 本地插件源没有可升级的，直接重装
            codex.cli(&["plugin", "add", gs(a, "id"), "--json"])?;
            notes.push(format!("Codex：重装了 {}", gs(a, "id")));
        }
    }
    let notes: Vec<String> = notes.into_iter().map(|n| format!("[{}] {n}", gs(&row, "key"))).collect();
    for n in &notes {
        log(n);
    }
    Ok(notes)
}

/// 锁定：不再检查、不再拉取更新，插件文件夹里的修改保留；链接被改坏了照样修。解锁后恢复。
pub fn act_lock(body: &Value) -> R<Vec<String>> {
    let mut cfg = load_config();
    let p = find_managed(&mut cfg, gs(body, "plugin"))?;
    p.locked = gb(body, "locked");
    let msg = if p.locked {
        format!("[{}] 已锁定：不再检查和拉取更新，插件文件夹里的修改会保留", p.id)
    } else {
        format!("[{}] 已解锁：恢复检查和更新", p.id)
    };
    save_config(&cfg)?;
    log(&msg);
    Ok(vec![msg])
}

/// 另存并还原：把插件文件夹整份复制出去（带 .git，本地提交也在），再还原成跟踪分支的版本，继续同步
pub fn act_save(body: &Value) -> R<Vec<String>> {
    let mut cfg = load_config();
    let p = find_managed(&mut cfg, gs(body, "plugin"))?.clone();
    let d = repo_dir(&p);
    fs::create_dir_all(&*SAVED_DIR)?;
    let dest = SAVED_DIR.join(format!("{}-{}", p.id, now().format("%Y%m%d-%H%M%S")));
    copy_dir(&d, &dest, &[])?;
    fetch(&p)?;
    git(&["checkout", "-f", "-B", &p.branch, &format!("origin/{}", p.branch)], Some(&d))?;
    git(&["clean", "-fd"], Some(&d))?; // 没被跟踪的新文件也在另存的那份里
    let mut notes = vec![format!("[{}] 修改另存到 {}", p.id, display(&dest)), format!("[{}] 插件文件夹还原成 {} 分支的版本", p.id, p.branch)];
    if p.locked {
        // 选了另存并还原，就是要继续同步
        find_managed(&mut cfg, &p.id)?.locked = false;
        save_config(&cfg)?;
        notes.push(format!("[{}] 已解锁", p.id));
    }
    for n in &notes {
        log(n);
    }
    if std::env::var_os("PLUGINHUB_NO_OPEN").is_none() {
        let _ = platform::shell_open(&display(&dest)); // 打开刚另存的文件夹（测试时用环境变量关掉）
    }
    notes.extend(op_update(Some(&p.id), false)?);
    Ok(notes)
}

/// 在资源管理器（访达）或浏览器里打开：插件文件夹、插件中心的文件夹，或网页链接
pub fn open_target(target: &str) -> R<()> {
    let folder = match target {
        "plugins_dir" => Some(&*PLUGINS_DIR),
        "hub_dir" => Some(&*HUB_DIR),
        "saved_dir" => Some(&*SAVED_DIR),
        _ => None,
    };
    if let Some(f) = folder {
        fs::create_dir_all(f)?;
        return platform::shell_open(&display(f));
    }
    if target.starts_with("http://") || target.starts_with("https://") {
        return platform::shell_open(target);
    }
    bail!("只能打开插件中心的文件夹或网页链接。")
}

fn action(name: &str, body: &Value) -> Option<R<Vec<String>>> {
    Some(match name {
        "check" => op_check(),
        "update" => {
            let plugin = gs(body, "plugin");
            let force = gb(body, "force");
            if plugin.is_empty() {
                // 全部更新时顺带刷新移植的副本
                op_update(None, force).map(|mut ns| {
                    ns.extend(refresh_ports());
                    ns
                })
            } else {
                op_update(Some(plugin), force)
            }
        }
        "add" => add_plugin(gs(body, "repo"), gs(body, "branch"), true, &body_apps(body)),
        "install" => act_install(body),
        "uninstall" => act_uninstall(body),
        "sync" => act_sync(body),
        "verify" => {
            let app = gs(body, "app");
            let apps: Option<Vec<String>> = if APPS.contains(&app) { Some(vec![app.to_string()]) } else { None };
            verify_plugin(gs(body, "key"), apps.as_deref(), false)
        }
        "auto" => set_auto_update(gb(body, "enabled"), body.get("interval_minutes").and_then(Value::as_i64).unwrap_or(60)),
        "guard" => set_guard(gb(body, "enabled")),
        "lock" => act_lock(body),
        "save" => act_save(body),
        "budget" => act_budget(),
        _ => return None,
    })
}

/// 页面和命令行共用的接口：name 是接口名（state、check、update…），返回 (HTTP 状态码, JSON)
pub fn dispatch(name: &str, body: &Value) -> Result<Value, (u16, Value)> {
    let fail = |code: u16, msg: String| (code, json!({"error": msg}));
    match name {
        "ping" => Ok(json!({"app": "pluginhub", "version": HUB_VERSION})),
        "state" => build_state().map_err(|e| {
            log(&format!("读取状态出错：{e}"));
            fail(500, format!("读取状态出错：{e}"))
        }),
        "probe" => probe_repo(gs(body, "repo")).map_err(|e| fail(400, e.to_string())),
        "open" => open_target(gs(body, "target")).map(|_| json!({"notes": []})).map_err(|e| fail(400, e.to_string())),
        _ => {
            let guard = match OP.try_lock() {
                Ok(g) => g,
                Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
                Err(_) => return Err(fail(409, "正在执行另一个操作，请稍等。".into())),
            };
            let result = match hub_lock(10) {
                Ok(_lock) => match action(name, body) {
                    Some(r) => r,
                    None => return Err(fail(404, "not found".into())),
                },
                Err(e) => Err(e),
            };
            drop(guard);
            match result {
                Ok(notes) => match build_state() {
                    Ok(state) => Ok(json!({"notes": notes, "state": state})),
                    Err(e) => Err(fail(500, format!("读取状态出错：{e}"))),
                },
                Err(e) => {
                    log(&format!("操作失败：{e}"));
                    Err((400, json!({"error": e.to_string(), "state": build_state().ok()})))
                }
            }
        }
    }
}
