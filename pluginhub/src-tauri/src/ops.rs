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
use crate::source;
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

fn update_one(cfg: &Config, p: &PluginCfg, m: &mut Map<String, Value>, claude: &ClaudeCode, codex: &Codex, force: bool) -> R<Vec<String>> {
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
    let root = p.root(&d);
    let man = read_manifests_at(&d, None, p.path.as_deref().unwrap_or(""));
    if p.target("claude") {
        for c in &man.claude_plugins {
            notes.extend(claude.link(&c.name, &root.join(&c.path))?.into_iter().map(|n| format!("{tag} Claude Code：{n}")));
        }
    }
    if p.target("codex") {
        if let Some(name) = &man.codex_name {
            notes.extend(codex.link(name, &root, force)?.into_iter().map(|n| format!("{tag} Codex：{n}")));
        }
    }
    // 从这个仓库里拿的技能：统一存放处的链接和两边的链接都对上
    notes.extend(crate::skills::relink_repo(cfg, &p.id, &d).into_iter().map(|n| format!("{tag} {n}")));
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
        match update_one(&cfg, p, m, &claude, &codex, force) {
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
        let root = p.root(&d);
        let man = read_manifests_at(&d, None, p.path.as_deref().unwrap_or(""));
        if p.target("claude") {
            out.extend(man.claude_plugins.iter().map(|c| Target { key: p.id.clone(), app: "claude", name: c.name.clone(), folder: root.join(&c.path) }));
        }
        if p.target("codex") {
            if let Some(name) = &man.codex_name {
                out.push(Target { key: p.id.clone(), app: "codex", name: name.clone(), folder: root.clone() });
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

/// ~/.yuwanplugins 里另有用途的名字，仓库不能占：skills 是统一存放技能的文件夹（anthropics/skills、openai/skills 都叫这个）
const RESERVED_IDS: &[&str] = &["skills"];

/// 管理列表里的 id，也是克隆在 ~/.yuwanplugins 里的文件夹名。这个仓库已经在管理就用它的；否则用仓库名，
/// 名字被占了（别的仓库、保留的名字、已有的文件夹）就带上 owner，再不行加序号
fn pick_id(src: &source::Source, cfg: &Config) -> String {
    if let Some(p) = cfg.plugins.iter().find(|p| source::same_repo(&p.repo, &src.repo)) {
        return p.id.clone();
    }
    let free = |id: &str| {
        if id.is_empty() || id.starts_with('.') || RESERVED_IDS.contains(&id.to_lowercase().as_str()) || cfg.plugins.iter().any(|p| p.id.eq_ignore_ascii_case(id)) {
            return false;
        }
        let d = PLUGINS_DIR.join(id);
        // 文件夹已经在了：是这个仓库以前留下的克隆才接着用，别的（移植的副本、别的仓库）不碰
        !d.exists() || source::same_repo(git_soft(&["config", "--get", "remote.origin.url"], Some(&d)).trim(), &src.repo)
    };
    let base = repo_id(&src.repo);
    let mut names = vec![base.clone()];
    if let Some((owner, repo)) = &src.github {
        names.push(repo_id(&format!("{owner}-{repo}")));
    }
    names.extend((2..30).map(|n| format!("{base}-{n}")));
    names.into_iter().find(|id| free(id)).unwrap_or(base)
}

pub fn repo_id(repo: &str) -> String {
    let trimmed = repo.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let last = last.rsplit(':').next().unwrap_or(last);
    Regex::new(r"[^\w.-]").unwrap().replace_all(last, "-").to_string()
}

fn json_version(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// 逐个细看的分支最多这么多：默认分支和最近有提交的，加上点名的、已经在跟的。有的仓库有几百个分支
const PROBE_BRANCHES: usize = 12;
/// 一个分支上最多列这么多技能
const PROBE_SKILLS: usize = 200;

/// 添加前先看看：粘进来的是什么（仓库、子目录、npx 命令），有哪些分支，每个分支上的插件清单和技能。
/// 只下载各分支最新提交里的小文件。
pub fn probe_repo(input: &str) -> R<Value> {
    let src = source::parse(input)?;
    let repo = src.repo.clone();
    let cfg = load_config();
    let pid = pick_id(&src, &cfg);
    let existing = cfg.plugins.iter().find(|p| p.id == pid).cloned();
    let managed_skills: Vec<String> = cfg.repo_skills(&pid).iter().filter_map(|s| s.path.clone()).collect();
    let local = crate::skills::local_index();
    fs::create_dir_all(&*TMP_DIR)?;
    let d = TMP_DIR.join(format!("probe-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    let result = (|| -> R<Value> {
        // 每个分支只取最新一个提交，大文件先不下载；插件清单和 SKILL.md 都很小
        git_net(&["clone", "--bare", "--depth=1", "--no-single-branch", "--filter=blob:limit=256k", &repo, &display(&d)], None, 300, Some(&d))?;
        let default = git_soft(&["symbolic-ref", "--short", "HEAD"], Some(&d)).trim().to_string();
        let fmt = "--format=%(refname:short)%1f%(objectname)%1f%(committerdate:iso-strict)%1f%(contents:subject)%1f%(committerdate:unix)";
        let mut heads: Vec<Vec<String>> = Vec::new();
        for line in git(&["for-each-ref", fmt, "refs/heads"], Some(&d))?.lines() {
            let mut parts: Vec<String> = line.split('\x1f').map(str::to_string).collect();
            parts.resize(5, String::new());
            if !parts[0].is_empty() {
                heads.push(parts);
            }
        }
        let names: Vec<String> = heads.iter().map(|h| h[0].clone()).collect();
        let (ref_sel, path, ref_note) = source::resolve_ref(&src, &names, &default);
        let mut notes: Vec<String> = ref_note.into_iter().collect();
        // 默认分支放最前，其余按最近提交排
        heads.sort_by_key(|h| std::cmp::Reverse(h[4].trim().parse::<i64>().unwrap_or(0)));
        heads.sort_by_key(|h| h[0] != default);
        let total = heads.len();
        let mut keep: Vec<Vec<String>> = heads.iter().take(PROBE_BRANCHES).cloned().collect();
        for must in [Some(ref_sel.clone()), existing.as_ref().map(|p| p.branch.clone())].into_iter().flatten() {
            if !keep.iter().any(|h| h[0] == must) {
                keep.extend(heads.iter().find(|h| h[0] == must).cloned());
            }
        }
        // 各分支上大多是同一批文件，内容按哈希只读一次
        let blobs: std::cell::RefCell<std::collections::HashMap<String, Option<String>>> = Default::default();
        let blob = |sha: &str| -> Option<String> {
            if let Some(hit) = blobs.borrow().get(sha) {
                return hit.clone();
            }
            let text = show_blob(&d, sha);
            blobs.borrow_mut().insert(sha.to_string(), text.clone());
            text
        };
        let mut branches = Vec::new();
        for h in keep {
            let (name, sha, date, subject) = (&h[0], &h[1], &h[2], &h[3]);
            let tree = list_tree(&d, name);
            let load = |rel: &str| tree.get(rel).and_then(|x| blob(x)).and_then(|t| serde_json::from_str::<Value>(&t).ok());
            let has = |rel: &str| tree.contains_key(rel);
            let root_man = manifests_with(&load, &has, "");
            // 命令里点名了插件源里的某个插件：只要它那个子目录
            let mut sub = path.clone();
            if let (None, Some(want)) = (&sub, &src.plugin) {
                match root_man.claude_plugins.iter().find(|c| c.name.eq_ignore_ascii_case(want)) {
                    Some(c) if c.path != "." => sub = Some(c.path.clone()),
                    Some(_) => {}
                    None if *name == ref_sel => {
                        let have: Vec<&str> = root_man.claude_plugins.iter().map(|c| c.name.as_str()).take(12).collect();
                        notes.push(if have.is_empty() { format!("这个仓库的插件源里没有叫 {want} 的插件") } else { format!("这个仓库的插件源里没有叫 {want} 的插件，有：{}", have.join("、")) });
                    }
                    None => {}
                }
            }
            // 指到了子目录：那里是插件就按那个插件算，是技能就记下来
            let at = sub.as_deref().map(|p| (manifests_with(&load, &has, p), has(&format!("{p}/SKILL.md"))));
            let (man, plugin_path) = match &at {
                Some((m, _)) if m.is_plugin() => (m.clone(), sub.clone()),
                _ => (root_man, None),
            };
            // 插件源里列的每个插件：说明、版本，文件夹里有没有 Codex 的清单
            let plugins: Vec<Value> = man
                .claude_plugins
                .iter()
                .map(|c| {
                    let base = [plugin_path.as_deref().unwrap_or(""), if c.path == "." { "" } else { c.path.as_str() }].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join("/");
                    let rel = |f: &str| if base.is_empty() { f.to_string() } else { format!("{base}/{f}") };
                    let pj = load(&rel(".claude-plugin/plugin.json")).unwrap_or(Value::Null);
                    json!({"name": c.name, "path": c.path, "description": gs(&pj, "description"), "version": json_version(pj.get("version")), "codex": has(&rel(CODEX_MANIFEST))})
                })
                .collect();
            let mut dirs: Vec<(String, &String)> = tree.iter().filter_map(|(f, x)| crate::skills::skill_dir_of(f).map(|dir| (dir, x))).collect();
            if let Some(p) = &path {
                dirs.retain(|(x, _)| x == p || x.starts_with(&format!("{p}/")));
            }
            let skills: Vec<Value> = dirs
                .iter()
                .take(PROBE_SKILLS)
                .map(|(dir, x)| {
                    let front = blob(x).and_then(|t| crate::skills::parse_front(&t)).unwrap_or_default();
                    let folder = dir.rsplit('/').next().unwrap_or("");
                    let fname = if !front.name.is_empty() {
                        front.name.clone()
                    } else if folder.is_empty() {
                        pid.clone()
                    } else {
                        folder.to_string()
                    };
                    let here = local.get(&fname.to_lowercase()).cloned().unwrap_or_default();
                    json!({"name": fname, "path": dir, "description": front.description, "version": front.version, "local": here})
                })
                .collect();
            branches.push(json!({"name": name, "sha": sha, "date": date, "subject": subject, "default": *name == default,
                                 "version": man.version, "title": man.title, "description": man.description,
                                 "claude": !man.claude_plugins.is_empty(), "codex": man.codex_name.is_some(),
                                 "plugins": plugins, "plugin_path": plugin_path,
                                 "at_path": at.as_ref().map(|(m, skill)| json!({"plugin": m.is_plugin(), "skill": skill})),
                                 "skills": skills, "skills_total": dirs.len()}));
        }
        // 建议的装法：点名了技能、链接指到技能、仓库里没有插件只有技能，就装技能；否则装插件
        let picked = branches.iter().find(|b| gs(b, "name") == ref_sel).or(branches.first());
        let mode = match picked {
            None => "none",
            Some(b) => {
                let is_plugin = gb(b, "claude") || gb(b, "codex");
                let at_skill = b.get("at_path").and_then(|a| a.get("skill")).and_then(Value::as_bool).unwrap_or(false);
                if src.wants_skills() || at_skill {
                    "skills"
                } else if is_plugin {
                    "plugin"
                } else if !ga(b, "skills").is_empty() {
                    "skills"
                } else {
                    "none"
                }
            }
        };
        let managed_as = existing.as_ref().map(|p| if APPS.iter().any(|a| p.target(a)) { "plugin" } else { "skills" });
        Ok(json!({
            "repo": repo, "web": src.web, "id": pid,
            "managed": existing.is_some(), "managed_as": managed_as,
            "managed_branch": existing.as_ref().map(|p| p.branch.clone()), "managed_skills": managed_skills,
            "mode": mode, "ref": ref_sel, "path": path, "notes": notes,
            "input": {"via": src.via.name(), "text": src.describe(), "skills": src.skills, "plugin": src.plugin, "agents": src.agents, "notes": src.notes},
            "branches": branches, "branches_total": total,
        }))
    })();
    let _ = rmtree(&d);
    result
}

/// 接管一个插件仓库，装到 apps 里列出的 app。repo 也可以是 npx skills add … 这样的命令，见 add_source
pub fn add_plugin(repo: &str, branch: &str, apply: bool, apps: &[String]) -> R<Vec<String>> {
    add_source(repo, branch, apply, apps, None, &[])
}

/// 一次添加要装什么：插件（在仓库的哪个子目录，None 是根目录），或者仓库里的几个技能文件夹
enum Plan {
    Plugin(Option<String>),
    Skills(Vec<String>),
}

/// 只给了一个地址或命令（命令行、脚本），没在页面上选过：看克隆下来的内容决定装什么。hint 是链接里带的子目录
fn decide(src: &source::Source, d: &Path, hint: Option<&str>) -> R<Plan> {
    let list = |v: &[(String, String)]| v.iter().take(12).map(|(n, _)| n.as_str()).collect::<Vec<_>>().join("、");
    let how = "在页面的添加里勾选，或者写成 npx skills add <仓库> --skill 名字";
    // 点名了技能（npx skills add … --skill a,b，或者 skills.sh 的页面）
    if !src.skills.is_empty() {
        let found = crate::skills::find_in_clone(d, hint);
        if found.is_empty() {
            bail!("这个仓库里没有技能（找不到 SKILL.md）。");
        }
        if src.skills.iter().any(|w| w == "*") {
            return Ok(Plan::Skills(found.into_iter().map(|(_, p)| p).collect()));
        }
        let mut picks = Vec::new();
        for want in &src.skills {
            let folder = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
            match found.iter().find(|(n, p)| n.eq_ignore_ascii_case(want) || folder(p).eq_ignore_ascii_case(want)) {
                Some((_, p)) => picks.push(p.clone()),
                None => bail!("仓库里没有叫 {want} 的技能。有：{}", list(&found)),
            }
        }
        return Ok(Plan::Skills(picks));
    }
    // 点名了插件源里的某个插件
    if let Some(want) = &src.plugin {
        let root = read_manifests_at(d, None, "");
        return match root.claude_plugins.iter().find(|c| c.name.eq_ignore_ascii_case(want)) {
            Some(c) => Ok(Plan::Plugin((c.path != ".").then(|| c.path.clone()))),
            None => bail!("这个仓库的插件源里没有叫 {want} 的插件。有：{}", root.claude_plugins.iter().map(|c| c.name.as_str()).take(12).collect::<Vec<_>>().join("、")),
        };
    }
    // 链接指到了子目录
    if let Some(h) = hint {
        if read_manifests_at(d, None, h).is_plugin() {
            return Ok(Plan::Plugin(Some(h.to_string())));
        }
        if d.join(h).join("SKILL.md").is_file() {
            return Ok(Plan::Skills(vec![h.to_string()]));
        }
        let found = crate::skills::find_in_clone(d, Some(h));
        return match found.len() {
            0 => bail!("{h} 里既没有插件清单，也没有 SKILL.md。"),
            1 => Ok(Plan::Skills(vec![found[0].1.clone()])),
            n => bail!("{h} 里有 {n} 个技能（{}）。{how}。", list(&found)),
        };
    }
    if !src.wants_skills() && read_manifests_at(d, None, "").is_plugin() {
        return Ok(Plan::Plugin(None));
    }
    let found = crate::skills::find_in_clone(d, None);
    match found.len() {
        0 if src.wants_skills() => bail!("这个仓库里没有技能（找不到 SKILL.md）。"),
        0 => bail!("这个仓库里既没有 .claude-plugin/plugin.json，也没有 .codex-plugin/plugin.json，也找不到 SKILL.md，不像插件或技能仓库。"),
        1 => Ok(Plan::Skills(vec![found[0].1.clone()])),
        n if src.wants_skills() => bail!("仓库里有 {n} 个技能（{}）。用 --skill 名字 指定要哪个（--all 是全部），或者在页面的添加里勾选。", list(&found)),
        n => bail!("这个仓库不是插件（没有插件清单），里面有 {n} 个技能（{}）。要装技能，{how}。", list(&found)),
    }
}

/// 添加。input 是仓库地址，也可以是 npx skills add … 这样的命令（见 source.rs）。
/// 页面上选好了的会传 path（插件在仓库里的子目录）或 skills（要装的技能文件夹）；都没传就看输入和仓库内容决定：
/// 是插件就接管成受管插件，是技能就装成统一存放的技能。两种情况仓库都克隆到 ~/.yuwanplugins/<id>，以后跟着分支更新。
pub fn add_source(input: &str, branch: &str, apply: bool, apps: &[String], path: Option<&str>, skills: &[String]) -> R<Vec<String>> {
    let src = source::parse(input)?;
    let repo = src.repo.clone();
    let clean = |s: &str| s.trim().trim_start_matches("./").trim_matches('/').to_string();
    let chosen = path.is_some() || !skills.is_empty();
    // 命令里指定了装到哪个 app（npx skills add … -a codex）的，没在页面上选过就照它的
    let apps: Vec<String> = if !chosen && !src.agents.is_empty() { apps.iter().filter(|a| src.agents.contains(a)).cloned().collect() } else { apps.to_vec() };
    if apps.is_empty() {
        bail!("至少选一个要安装的 app。");
    }
    let mut cfg = load_config();
    let pid = pick_id(&src, &cfg);
    let existing = cfg.plugins.iter().find(|p| p.id == pid).cloned();
    let mut notes: Vec<String> = Vec::new();
    // 跟哪个分支；链接里带的子目录（hint）
    let (branch, hint) = match (&existing, branch.trim()) {
        (Some(p), b) => {
            // 已经在管理的仓库只有一份克隆，跟着一个分支
            let other = |x: &str| format!("{pid} 已经跟着 {} 分支在管理了，不能同时再跟 {x} 分支。", p.branch);
            if !b.is_empty() && b != p.branch {
                bail!("{}", other(b));
            }
            let mut hint = None;
            if !chosen {
                if let Some(tree) = src.tree.as_deref() {
                    let (r, sub, matched) = source::split_tree(tree, std::slice::from_ref(&p.branch));
                    if !matched {
                        bail!("{}", other(&r));
                    }
                    hint = if src.file { sub.and_then(|s| s.rsplit_once('/').map(|(d, _)| d.to_string())) } else { sub };
                } else if let Some(h) = src.ref_hint.as_deref().filter(|h| *h != p.branch) {
                    bail!("{}", other(h));
                }
            }
            (p.branch.clone(), hint)
        }
        (None, b) if !b.is_empty() => {
            let b = validate_branch(b)?;
            let hint = if chosen { None } else { source::resolve_ref(&src, std::slice::from_ref(&b), &b).1 };
            (b, hint)
        }
        (None, _) if src.tree.is_some() || src.ref_hint.is_some() => {
            let (default, heads) = remote_heads(&repo)?;
            let (b, hint, note) = source::resolve_ref(&src, &heads, &default);
            notes.extend(note);
            (validate_branch(&b)?, hint)
        }
        (None, _) => (validate_branch(&default_branch(&repo)?)?, None),
    };
    let is_new = existing.is_none();
    if !is_new {
        // 查询看的是远端最新的提交，本机的克隆可能落后：先同步一次。锁定的、里面有修改的不会拉，照旧用现有的文件
        notes.extend(op_update(Some(&pid), false)?);
    }
    let mut p = existing.clone().unwrap_or_else(|| PluginCfg {
        id: pid.clone(),
        repo: repo.clone(),
        branch: branch.clone(),
        targets: Some(APPS.iter().map(|a| (a.to_string(), false)).collect()),
        locked: false,
        path: None,
        extra: Map::new(),
    });
    let d = ensure_clone(&p)?;
    // 记进配置之前出了错，刚克隆的不留
    let prepared = (|| -> R<(Plan, Vec<String>, Vec<String>)> {
        let plan = if !skills.is_empty() {
            Plan::Skills(skills.iter().map(|s| clean(s)).collect())
        } else if let Some(sub) = path {
            Plan::Plugin(Some(clean(sub)).filter(|s| !s.is_empty() && s != "."))
        } else {
            decide(&src, &d, hint.as_deref())?
        };
        match &plan {
            Plan::Plugin(sub) => {
                if existing.as_ref().is_some_and(|old| APPS.iter().any(|a| old.target(a))) {
                    bail!("{pid} 已经在管理列表里了。");
                }
                if !read_manifests_at(&d, None, sub.as_deref().unwrap_or("")).is_plugin() {
                    let place = sub.as_deref().map(|s| format!("子目录 {s} ")).unwrap_or_else(|| "这个仓库".into());
                    bail!("{place}里既没有带 plugin.json 的 .claude-plugin，也没有 .codex-plugin/plugin.json，不像插件。");
                }
                p.path = sub.clone();
                for x in APPS {
                    p.set_target(x, apps.iter().any(|a| a == x));
                }
                match cfg.plugins.iter_mut().find(|x| x.id == pid) {
                    Some(slot) => *slot = p.clone(), // 原来只拿了技能的仓库，现在也装成插件
                    None => cfg.plugins.push(p.clone()),
                }
                let place = sub.as_deref().map(|s| format!("，子目录 {s}")).unwrap_or_default();
                Ok((plan, vec![format!("[{pid}] 开始管理（{branch} 分支{place}）")], Vec::new()))
            }
            Plan::Skills(picks) => {
                // 仓库克隆下来但不装成插件（两边的目标都是关的），选中的技能链接进统一存放处和两边
                let mut done = Vec::new();
                if is_new {
                    cfg.plugins.push(p.clone());
                    done.push(format!("[{pid}] 开始管理（{branch} 分支，只拿里面的技能）"));
                }
                let (names, linked) = crate::skills::add_from_repo(&mut cfg, &pid, &d, picks, &apps)?;
                done.extend(linked);
                Ok((plan, done, names))
            }
        }
    })();
    let (plan, done, names) = match prepared {
        Ok(x) => x,
        Err(e) => {
            if is_new {
                let _ = rmtree(&d);
            }
            return Err(e);
        }
    };
    remember_proxy(&mut cfg);
    save_config(&cfg)?;
    for n in &done {
        log(n);
    }
    notes.extend(done);
    if apply {
        match plan {
            Plan::Plugin(_) => {
                notes.extend(op_update(Some(&pid), false)?);
                notes.extend(verify_plugin(&pid, Some(&apps), true)?);
            }
            Plan::Skills(_) => {
                for name in &names {
                    notes.extend(verify_plugin(&format!("skill:{name}"), Some(&apps), true)?);
                }
            }
        }
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

/// 收编：本机原有的独立 skill 挪进 ~/.yuwanplugins/skills 统一存放，原位置换成链接，不装到别的 app。
/// 一次可以收编好几个（keys），一个出错不耽误别的，出错的写在结果里。
pub fn act_adopt(body: &Value) -> R<Vec<String>> {
    let keys: Vec<String> = ga(body, "keys").iter().filter_map(Value::as_str).map(str::to_string).collect();
    if keys.is_empty() {
        bail!("没有要收编的技能。");
    }
    let mut notes = Vec::new();
    let mut failed = 0;
    for key in &keys {
        let row = find_row(key)?;
        if gs(&row, "kind") != "skill" || gb(&row, "official") {
            bail!("{key} 不是本机原有的独立技能。");
        }
        if gb(&row["skill"], "ours") {
            continue; // 已经归插件中心管了
        }
        match crate::skills::install(&row, &[]) {
            Ok(ns) => notes.extend(ns),
            Err(e) => {
                failed += 1;
                let n = format!("[{}] 没收编：{e}", gs(&row, "name"));
                log(&n);
                notes.push(n);
            }
        }
    }
    if failed > 0 && failed == keys.len() {
        bail!("{}", notes.join("\n"));
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
            if cfg.repo_skills(gs(v, "id")).is_empty() {
                // 受管但哪边都没装上，也一并不再管理
                let p = cfg.plugins.remove(i);
                save_config(&cfg)?;
                notes.push(format!("[{}] 插件中心不再管理它，插件文件夹挪到 {}", gs(&row, "key"), retire(&p)?));
                log(notes.last().expect("pushed"));
            } else if APPS.iter().any(|a| cfg.plugins[i].target(a)) {
                // 仓库里还有技能在用：克隆留着，只是不再装成插件
                for a in APPS {
                    cfg.plugins[i].set_target(a, false);
                }
                cfg.plugins[i].path = None;
                save_config(&cfg)?;
            }
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
                if cfg.repo_skills(&removed.id).is_empty() {
                    cfg.plugins.retain(|x| x.id != removed.id);
                    notes.push(format!("两边都卸掉了，插件中心不再管理它，插件文件夹挪到 {}", retire(&removed)?));
                } else {
                    // 仓库里还有技能在用：克隆留着，只是不再装成插件
                    if let Some(x) = cfg.plugins.iter_mut().find(|x| x.id == removed.id) {
                        for y in APPS {
                            x.set_target(y, false);
                        }
                        x.path = None;
                    }
                    notes.push("两边都卸掉了；这个仓库里还有技能在用，克隆留着继续跟着分支更新".into());
                }
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
        // 从仓库装的技能先拉取仓库；然后把该有的链接补上，再做一遍真实检查
        let mut notes = Vec::new();
        if let Some(m) = row.get("managed").filter(|m| m.is_object()) {
            notes.extend(op_update(Some(gs(m, "id")), false)?);
        }
        notes.extend(crate::skills::sync(&row)?);
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
    // 页面上点插件或技能的名字打开它的存放位置：只开插件中心自己的和两边 app 的目录，别处的路径不开
    let p = std::path::Path::new(target);
    if p.is_absolute() && p.is_dir() && [&*PLUGINS_DIR, &*HUB_DIR, &*CLAUDE_HOME, &*CODEX_HOME].iter().any(|root| under(p, root)) {
        return platform::shell_open(target);
    }
    bail!("只能打开插件中心的文件夹、插件和技能的存放位置，或网页链接。")
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
        "add" => {
            let skills: Vec<String> = ga(body, "skills").iter().filter_map(Value::as_str).map(str::to_string).collect();
            add_source(gs(body, "repo"), gs(body, "branch"), true, &body_apps(body), body.get("path").and_then(Value::as_str), &skills)
        }
        "install" => act_install(body),
        "adopt" => act_adopt(body),
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
        "search" => crate::registry::search(gs(body, "q")).map(|list| json!({"skills": list})).map_err(|e| fail(400, e.to_string())),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn managed(id: &str, repo: &str) -> PluginCfg {
        PluginCfg { id: id.into(), repo: repo.into(), branch: "main".into(), targets: None, locked: false, path: None, extra: Map::new() }
    }

    #[test]
    fn ids_avoid_reserved_names_and_other_repos() {
        let mut cfg = Config::default();
        // skills 是统一存放技能的文件夹，仓库叫这个名字也不能占
        assert_eq!(pick_id(&source::parse("anthropics/skills").unwrap(), &cfg), "anthropics-skills");
        assert_eq!(pick_id(&source::parse("https://example.com/team/Skills.git").unwrap(), &cfg), "Skills-2");
        // 已经在管理的仓库，换个写法还是它
        cfg.plugins.push(managed("zz-pick-a", "git@github.com:Owner/zz-pick-a.git"));
        assert_eq!(pick_id(&source::parse("https://github.com/owner/zz-pick-a").unwrap(), &cfg), "zz-pick-a");
        // 名字被别的仓库占了就带上 owner
        assert_eq!(pick_id(&source::parse("other/zz-pick-a").unwrap(), &cfg), "other-zz-pick-a");
        assert_eq!(pick_id(&source::parse("npx skills add someone/zz-pick-b --skill x").unwrap(), &cfg), "zz-pick-b");
    }
}
