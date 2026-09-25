//! 技能库：按需的插件和技能不装进 app，只列在插件中心自带的 skill-library 里，用到时模型再去读。
//!
//! skill-library 放在 ~/.pluginhub/library，两边的 skills 文件夹都链接到这里。每次会话 app 只加载它开头的一句说明；
//! 模型要用时顺着读下去：SKILL.md（分组）→ groups/<组>.md（每个技能的说明和 SKILL.md 的位置）→ 技能本身。
//! 插件和技能的文件原样不动。
//!
//! 两处实测过的限制：Codex 会把技能文件夹里嵌套的 SKILL.md 也当成技能，所以这里只放普通的 .md；
//! Claude Code 读项目外的文件要许可，所以在它的 settings.json 里加上读技能库和 ~/.yuwanplugins 的规则。
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::bail;
use crate::codex::Codex;
use crate::gitx::{read_manifests, repo_dir};
use crate::skills::{app_dir, read_front, SKILLS_DIR};
use crate::store::*;
use crate::usage::{merge_projects, scan_usage, ProjMap, Usage};
use crate::util::*;
use crate::view::skill_dirs;

/// 技能库在两边 skills 文件夹里的名字，也是 SKILL.md 里的 name
pub const NAME: &str = "skill-library";
pub static LIB_DIR: LazyLock<PathBuf> = LazyLock::new(|| HUB_DIR.join("library"));
/// 说明最长 1024 个字符（超了 Codex 不加载），留点余地
const DESC_MAX: usize = 1000;
/// 「用过的项目」最多列几个
const MAX_PROJECTS: usize = 15;

// ---------------------------------------------------------------- 技能和常驻的开销

/// 一个技能每次会话常驻的开销（粗估 token）：名字加说明。按 Claude Code 报的数校准过：
/// 中文大约一字一个半 token，其余大约四个字符一个，每条再加一点格式。
pub fn tokens(name: &str, description: &str) -> u64 {
    let text = format!("{name}: {description}");
    let cjk = text.chars().filter(|c| *c as u32 >= 0x2E80).count() as u64;
    let other = text.chars().count() as u64 - cjk;
    cjk * 3 / 2 + other / 4 + 8
}

pub struct Leaf {
    pub name: String,
    pub description: String,
    pub md: PathBuf,
}

pub fn cost(leaves: &[Leaf]) -> u64 {
    leaves.iter().map(|l| tokens(&l.name, &l.description)).sum()
}

/// 这些文件夹里的技能，同一个文件夹只算一次。路径写成规整的绝对路径（插件在仓库根目录时清单里写的是 .）
pub fn leaves(dirs: Vec<PathBuf>) -> Vec<Leaf> {
    let mut dirs: Vec<PathBuf> = dirs.into_iter().map(|d| std::path::absolute(&d).unwrap_or(d)).collect();
    dirs.sort_by_key(|d| norm(d));
    dirs.dedup_by_key(|d| norm(d));
    dirs.iter()
        .filter_map(|d| {
            let md = d.join("SKILL.md");
            let f = read_front(&md)?;
            Some(Leaf { name: if f.name.is_empty() { file_name(d) } else { f.name }, description: f.description, md })
        })
        .collect()
}

/// 受管插件文件夹里的技能：两边清单指到的 skills 文件夹合在一起
pub fn repo_leaves(d: &Path) -> Vec<Leaf> {
    let man = read_manifests(d, None);
    let mut dirs: Vec<PathBuf> = man.claude_plugins.iter().flat_map(|c| skill_dirs(&d.join(&c.path), "claude")).collect();
    dirs.extend(skill_dirs(d, "codex"));
    leaves(dirs)
}

/// 按需时不会生效的东西：只有技能能从技能库读，钩子、命令、子代理和 MCP 服务都要装进 app 才有
pub fn lost_on_demand(d: &Path) -> Vec<&'static str> {
    let man = read_manifests(d, None);
    let mut folders: Vec<PathBuf> = man.claude_plugins.iter().map(|c| d.join(&c.path)).collect();
    folders.push(d.to_path_buf());
    let has = |file: &str, key: &str| {
        folders.iter().any(|f| f.join(file).exists() || read_obj(&f.join(".claude-plugin").join("plugin.json")).contains_key(key))
    };
    [("hooks/hooks.json", "hooks", "钩子"), ("commands", "commands", "斜杠命令"), ("agents", "agents", "子代理"), (".mcp.json", "mcpServers", "MCP 服务")]
        .into_iter()
        .filter(|(file, key, _)| has(file, key))
        .map(|(_, _, what)| what)
        .collect()
}

// ---------------------------------------------------------------- 分组

/// 技能库里的一组：一个按需的插件，或者按需的独立技能（统一放在 skills 这组）
pub struct Group {
    pub id: String,
    pub about: String,
    /// 插件的文件夹，它的文件里写的 ${CLAUDE_PLUGIN_ROOT} 就是这里；独立的技能没有
    pub folder: Option<PathBuf>,
    pub leaves: Vec<Leaf>,
}

pub fn groups(cfg: &Config) -> Vec<Group> {
    let mut out: Vec<Group> = cfg
        .plugins
        .iter()
        .filter(|p| p.on_demand)
        .map(|p| {
            let d = repo_dir(p);
            Group { id: p.id.clone(), about: read_manifests(&d, None).description, leaves: repo_leaves(&d), folder: Some(d) }
        })
        .collect();
    let loose = leaves(cfg.skills.iter().filter(|s| s.on_demand).map(|s| SKILLS_DIR.join(&s.dir)).collect());
    out.push(Group { id: "skills".into(), about: "Standalone skills.".into(), folder: None, leaves: loose });
    out.retain(|g| !g.leaves.is_empty());
    out
}

/// 用过这个受管插件的项目（两个 app 合起来，按需时读文件也算）。id 是它在 ~/.yuwanplugins 里的文件夹名
pub fn plugin_projects(id: &str, usage: &Usage) -> ProjMap {
    let man = read_manifests(&PLUGINS_DIR.join(id), None);
    let yuwan = format!("yuwan:{id}");
    let codex_id = man.codex_name.map(|n| format!("{n}@{}", Codex::marketplace_name()));
    let mut maps = vec![usage.get("claude", &yuwan), usage.get("codex", &yuwan)];
    maps.extend(man.claude_plugins.iter().map(|c| usage.get("claude", &c.name)));
    if let Some(cid) = &codex_id {
        maps.push(usage.get("codex", cid));
    }
    merge_projects(&maps)
}

/// 用过这个独立技能的项目：名字和文件夹名都算
pub fn skill_projects(s: &SkillCfg, usage: &Usage) -> ProjMap {
    let mut keys = vec![format!("skill:{}", s.name), format!("skill:{}", s.dir)];
    keys.dedup();
    merge_projects(&APPS.iter().flat_map(|app| keys.iter().map(move |k| usage.get(app, k))).collect::<Vec<_>>())
}

/// 用过技能库里这些东西的项目，最近用过的在前：(项目路径, 用过的组或技能)
fn used_before(cfg: &Config, usage: &Usage) -> Vec<(String, Vec<String>)> {
    let mut items: Vec<(String, ProjMap)> = cfg.plugins.iter().filter(|p| p.on_demand).map(|p| (p.id.clone(), plugin_projects(&p.id, usage))).collect();
    items.extend(cfg.skills.iter().filter(|s| s.on_demand).map(|s| (s.name.clone(), skill_projects(s, usage))));
    let mut by: BTreeMap<String, (String, f64, Vec<String>)> = BTreeMap::new();
    for (label, projects) in items {
        for (key, row) in projects {
            let e = by.entry(key).or_insert_with(|| (row.path.clone(), 0.0, Vec::new()));
            e.1 = e.1.max(row.last);
            e.2.push(label.clone());
        }
    }
    let mut list: Vec<(String, f64, Vec<String>)> = by.into_values().collect();
    list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    list.into_iter().take(MAX_PROJECTS).map(|(path, _, names)| (path, names)).collect()
}

// ---------------------------------------------------------------- 写出技能库

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

/// 说明里最前面那句，去掉 Use when 这类开头，当作「它是干什么的」的提示
fn hint(text: &str, max: usize) -> String {
    let t = one_line(text);
    let lower = t.to_lowercase();
    let start = ["use this skill when ", "use this when ", "use whenever ", "use when ", "use after ", "use before ", "use for ", "use to "]
        .iter()
        .find(|p| lower.starts_with(*p))
        .map(|p| p.len())
        .unwrap_or(0);
    let t = &t[start..];
    let end = [". ", "。", "; ", "；", " - ", " — "].iter().filter_map(|s| t.find(s)).min().unwrap_or(t.len());
    clip(t[..end].trim_end_matches('.'), max)
}

/// 开头的说明。每次会话 app 只加载这一句，模型靠它判断什么时候打开技能库，所以列出每个技能和它是干什么的；
/// 放不下就逐级缩短提示，最后只列组名。实测光列名字不够：名字看不出用途时，两边的模型都不会来打开。
fn description(groups: &[Group]) -> String {
    let n: usize = groups.iter().map(|g| g.leaves.len()).sum();
    let head = format!(
        "Index of {n} installed skill{} kept out of the prompt to save context. \
         Check it before starting a task that may match one of them, or when the user asks for a skill that isn't loaded.",
        if n == 1 { "" } else { "s" }
    );
    for (about, each) in [(60, 60), (60, 30), (60, 0), (0, 0)] {
        let parts: Vec<String> = groups
            .iter()
            .map(|g| {
                let about = if about > 0 && g.folder.is_some() && !g.about.is_empty() { format!(" ({})", hint(&g.about, about)) } else { String::new() };
                let skills: Vec<String> = g
                    .leaves
                    .iter()
                    .map(|l| {
                        let h = if each > 0 { hint(&l.description, each) } else { String::new() };
                        if h.is_empty() { l.name.clone() } else { format!("{} ({h})", l.name) }
                    })
                    .collect();
                format!("{}{about}: {}", g.id, skills.join(", "))
            })
            .collect();
        let text = format!("{head} {}.", parts.join("; "));
        if text.chars().count() <= DESC_MAX {
            return text;
        }
    }
    let brief: Vec<String> = groups.iter().map(|g| format!("{} ({})", g.id, g.leaves.len())).collect();
    clip(&format!("{head} Groups: {}.", brief.join(", ")), DESC_MAX)
}

fn group_file(id: &str) -> PathBuf {
    LIB_DIR.join("groups").join(format!("{id}.md"))
}

fn index(groups: &[Group], used: &[(String, Vec<String>)]) -> String {
    let desc = description(groups).replace('\\', "\\\\").replace('"', "\\\"");
    let mut t = format!(
        "---\nname: {NAME}\ndescription: \"{desc}\"\n---\n\n# Skill library\n\n\
         The skills listed here are installed but not loaded, to keep the prompt small. To use one:\n\n\
         1. Read the group file that fits the task. It gives each skill's description and the path of its SKILL.md.\n\
         2. Read that SKILL.md and follow it as if it had been loaded. Paths in it are relative to its own folder, \
         and skills it names are in the same group.\n\
         3. Tell the user which skill you used.\n\n## Groups\n\n"
    );
    for g in groups {
        let n = g.leaves.len();
        let about = clip(&one_line(&g.about), 160);
        t += &format!("- {} ({n} skill{}): {about}\n  {}\n", g.id, if n == 1 { "" } else { "s" }, display(&group_file(&g.id)));
    }
    if !used.is_empty() {
        t += "\n## Used before\n\nProjects where these were used, most recent first. If the current folder is one of them, try its skills first.\n\n";
        for (path, names) in used {
            t += &format!("- {path}: {}\n", names.join(", "));
        }
    }
    t
}

fn group_text(g: &Group) -> String {
    let mut t = format!("# {}\n\n", g.id);
    if !g.about.is_empty() {
        t += &format!("{}\n\n", one_line(&g.about));
    }
    if let Some(f) = &g.folder {
        t += &format!("Plugin folder: {}\nWhere its files say ${{CLAUDE_PLUGIN_ROOT}}, they mean this folder.\n\n", display(f));
    }
    for l in &g.leaves {
        t += &format!("- {}: {}\n  {}\n", l.name, one_line(&l.description), display(&l.md));
    }
    t
}

/// 写出技能库的文件：内容没变的不重写，已经不在库里的组删掉
fn write_files(groups: &[Group], used: &[(String, Vec<String>)]) -> R<()> {
    let mut files: Vec<(PathBuf, String)> = groups.iter().map(|g| (group_file(&g.id), group_text(g))).collect();
    files.push((LIB_DIR.join("SKILL.md"), index(groups, used)));
    let dir = LIB_DIR.join("groups");
    fs::create_dir_all(&dir)?;
    for e in fs::read_dir(&dir)?.flatten() {
        if !files.iter().any(|(p, _)| same_path(Some(p), Some(&e.path()))) {
            remove_path(&e.path())?;
        }
    }
    for (p, text) in &files {
        if fs::read_to_string(p).ok().as_deref() != Some(text.as_str()) {
            fs::write(p, text)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- 装进两边

fn app_link(app: &str) -> PathBuf {
    app_dir(app).join(NAME)
}

/// 这个 app 装了（配置文件夹在）才放技能库
fn has_app(app: &str) -> bool {
    (if app == "claude" { &*CLAUDE_HOME } else { &*CODEX_HOME }).is_dir()
}

fn settings_path() -> PathBuf {
    CLAUDE_HOME.join("settings.json")
}

/// 权限规则里的路径写法：家目录下的写成 ~/xxx，别处的写成 //c/xxx
fn rule_path(p: &Path) -> String {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    if let Ok(rel) = abs.strip_prefix(&*HOME) {
        return format!("~/{}", rel.to_string_lossy().replace('\\', "/"));
    }
    let s = display(&abs).replace('\\', "/");
    match s.split_once(":/") {
        Some((drive, rest)) => format!("//{}/{rest}", drive.to_lowercase()),
        None => s,
    }
}

/// Claude Code 读这些地方不用每次都问：技能库（真实位置和链接的位置各一条）和 ~/.yuwanplugins
fn rules() -> Vec<String> {
    [LIB_DIR.clone(), app_link("claude"), PLUGINS_DIR.clone()].iter().map(|p| format!("Read({}/**)", rule_path(p))).collect()
}

fn rules_ok() -> bool {
    let data = read_obj(&settings_path());
    let have: Vec<&str> = data.get("permissions").and_then(|p| p.get("allow")).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    rules().iter().all(|r| have.contains(&r.as_str()))
}

/// 在 permissions.allow 里加上（on）或去掉这几条规则，别的内容不动；因为这个才有的空 allow 和 permissions 也去掉。返回改没改。
fn edit_rules(data: &mut Map<String, Value>, rules: &[String], on: bool) -> R<bool> {
    let perms = data.entry("permissions").or_insert_with(|| json!({}));
    let Some(perms) = perms.as_object_mut() else { bail!("settings.json 里的 permissions 不是对象，没有改它。") };
    let allow = perms.entry("allow").or_insert_with(|| json!([]));
    let Some(list) = allow.as_array_mut() else { bail!("settings.json 里的 permissions.allow 不是数组，没有改它。") };
    let before = list.len();
    if on {
        for r in rules {
            if !list.iter().any(|x| x.as_str() == Some(r.as_str())) {
                list.push(Value::from(r.as_str()));
            }
        }
    } else {
        list.retain(|x| !x.as_str().is_some_and(|s| rules.iter().any(|r| r == s)));
    }
    let changed = list.len() != before;
    if list.is_empty() {
        perms.remove("allow");
    }
    if perms.is_empty() {
        data.remove("permissions");
    }
    Ok(changed)
}

fn set_rules(on: bool) -> R<bool> {
    let path = settings_path();
    let mut data = match read_json(&path) {
        Some(Value::Object(m)) => m,
        None if !path.exists() => Map::new(),
        _ => bail!("{} 不是正常的 JSON，没有改它。", display(&path)),
    };
    let changed = edit_rules(&mut data, &rules(), on)?;
    if changed {
        write_json(&path, &Value::Object(data))?;
    }
    Ok(changed)
}

/// 按配置重新生成技能库并装进两边；没有按需的了就整个撤掉。返回每处改动：(app, 说明)
pub fn refresh() -> R<Vec<(&'static str, String)>> {
    let cfg = load_config();
    let groups = groups(&cfg);
    let mut out = Vec::new();
    if groups.is_empty() {
        for app in APPS {
            let link = app_link(app);
            if links_to(&link, &LIB_DIR) {
                fs::remove_dir(&link)?;
                out.push((app, format!("没有按需的了，去掉链接 {}", display(&link))));
            }
        }
        if has_app("claude") && set_rules(false)? {
            out.push(("claude", "从 settings.json 里去掉读取技能库的许可".to_string()));
        }
        rmtree(&LIB_DIR)?;
        return Ok(out);
    }
    write_files(&groups, &used_before(&cfg, &scan_usage()))?;
    for app in APPS.into_iter().filter(|a| has_app(a)) {
        let link = app_link(app);
        if make_junction(&link, &LIB_DIR)? {
            out.push((app, format!("链接 {} → {}", display(&link), display(&LIB_DIR))));
        }
    }
    if has_app("claude") && set_rules(true)? {
        out.push(("claude", format!("在 settings.json 里加上读取许可 {}", rules().join("、"))));
    }
    Ok(out)
}

// ---------------------------------------------------------------- 状态和真实检查

/// 页面上显示的技能库情况。rows 是插件和技能的行，用来算按需省下了多少。
pub fn status(rows: &[Value], st: &State) -> Value {
    let groups = groups(&load_config());
    let on = !groups.is_empty();
    let apps: Vec<Value> = APPS
        .into_iter()
        .filter(|a| has_app(a))
        .map(|app| {
            let link = app_link(app);
            let mut problems = Vec::new();
            if on && !links_to(&link, &LIB_DIR) {
                problems.push(if link.exists() { format!("{} 不是指向技能库的链接", display(&link)) } else { format!("{} 这个链接不见了", display(&link)) });
            }
            if on && app == "claude" && !rules_ok() {
                problems.push(format!("settings.json 里没有读取技能库的许可，读的时候会一次次问（{}）", changed_at(&settings_path())));
            }
            let key = format!("{app}:{NAME}");
            json!({"app": app, "linked": links_to(&link, &LIB_DIR), "problems": problems,
                   "verify": st.get("verify").and_then(|v| v.get(&key)).cloned().unwrap_or(Value::Null),
                   "repair": st.get("repairs").and_then(|v| v.get(&key)).cloned().unwrap_or(Value::Null)})
        })
        .collect();
    let saved: u64 = rows.iter().filter(|r| gb(r, "on_demand")).filter_map(|r| r.get("cost").and_then(Value::as_u64)).sum();
    json!({
        "on": on, "name": NAME, "folder": display(&LIB_DIR), "groups": groups.len(),
        "skills": groups.iter().map(|g| g.leaves.len()).sum::<usize>(),
        "cost": if on { tokens(NAME, &description(&groups)) } else { 0 }, "saved": saved, "apps": apps,
    })
}

/// Claude Code 没有列出技能的命令，只能核对链接、SKILL.md 和读取许可
fn check_claude() -> (bool, String) {
    let link = app_link("claude");
    if !links_to(&link, &LIB_DIR) {
        return (false, format!("{} 不是指向 {} 的链接", display(&link), display(&LIB_DIR)));
    }
    if !read_front(&link.join("SKILL.md")).is_some_and(|f| f.name == NAME && !f.description.is_empty()) {
        return (false, "技能库的 SKILL.md 读不了，或者开头格式不对".into());
    }
    if !rules_ok() {
        return (false, "settings.json 里没有读取技能库的许可，Claude Code 读的时候会一次次问".into());
    }
    (true, "链接、SKILL.md 和读取许可都在（Claude Code 没有列出技能的命令，只能核对文件）".into())
}

/// Codex 的模型提示里要有技能库，按需的技能不能还在里面，不然就没省下
fn check_codex(codex: &mut Codex, groups: &[Group]) -> R<(bool, String)> {
    let (ok, detail) = codex.verify_skill(NAME, Some(LIB_DIR.as_path()))?;
    if !ok {
        return Ok((false, detail));
    }
    let re = Regex::new(r"(?m)\(file: (.+?)\)\s*$").unwrap();
    let text = codex.prompt_text()?.to_string();
    let seen: Vec<String> = re.captures_iter(&text).map(|m| norm(Path::new(m[1].trim()))).collect();
    let loaded: Vec<&str> = groups.iter().flat_map(|g| &g.leaves).filter(|l| seen.contains(&norm(&l.md))).map(|l| l.name.as_str()).collect();
    if !loaded.is_empty() {
        return Ok((false, format!("按需的技能还在模型提示里：{}", loaded.join("、"))));
    }
    let n: usize = groups.iter().map(|g| g.leaves.len()).sum();
    Ok((true, format!("模型提示里有技能库，按需的 {n} 个技能都不在提示里")))
}

/// 真实检查技能库，结果记进 state 的 verify。返回每个 app 的 (app, 通过没有, 说明)
pub fn verify(codex: &mut Codex, st: &mut State) -> Vec<(&'static str, bool, String)> {
    let groups = groups(&load_config());
    let mut out = Vec::new();
    if groups.is_empty() {
        return out;
    }
    for app in APPS.into_iter().filter(|a| has_app(a)) {
        let (ok, detail) = if app == "claude" { check_claude() } else { check_codex(codex, &groups).unwrap_or_else(|e| (false, format!("检查时出错：{e}"))) };
        sub(st, "verify").insert(format!("{app}:{NAME}"), json!({"ok": ok, "detail": detail, "at": stamp()}));
        out.push((app, ok, detail));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str, names: &[&str]) -> Group {
        let leaves = names.iter().map(|n| Leaf { name: n.to_string(), description: String::new(), md: PathBuf::from(format!("{n}/SKILL.md")) }).collect();
        Group { id: id.into(), about: String::new(), folder: None, leaves }
    }

    #[test]
    fn tokens_count_chinese_heavier() {
        assert_eq!(tokens("a", "bcdefgh"), 10); // "a: bcdefgh" 十个字符 → 2，加 8
        assert_eq!(tokens("a", "中文"), 3 + 8); // 两个汉字算 3，"a: " 不到 4 个字符算 0
    }

    #[test]
    fn hints_take_the_first_clause() {
        assert_eq!(hint("Use when writing release notes or a changelog entry for a version.", 60), "writing release notes or a changelog entry for a version");
        assert_eq!(hint("Use before writing any code - decide whether it needs to exist.", 60), "writing any code");
        assert_eq!(hint("DCLH 的统一入口。用户说 dclh 时触发", 60), "DCLH 的统一入口");
        assert_eq!(hint("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn description_lists_names_then_falls_back_to_groups() {
        let d = description(&[group("dclh", &["writing", "auditing"]), group("skills", &["autoresearch"])]);
        assert!(d.starts_with("Index of 3 installed skills"));
        assert!(d.ends_with("dclh: writing, auditing; skills: autoresearch."));
        let many: Vec<String> = (0..80).map(|i| format!("a-rather-long-skill-name-{i}")).collect();
        let names: Vec<&str> = many.iter().map(String::as_str).collect();
        let d = description(&[group("big", &names)]);
        assert!(d.chars().count() <= DESC_MAX && d.ends_with("Groups: big (80)."));
    }

    #[test]
    fn index_front_matter_reads_back() {
        let dir = std::env::temp_dir().join(format!("pluginhub-lib-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let mut g = group("dclh", &["say-\"hi\"", "c:\\x"]);
        g.about = "关于".into();
        fs::write(dir.join("SKILL.md"), index(&[g], &[("D:\\proj".into(), vec!["dclh".into()])])).unwrap();
        let f = read_front(&dir.join("SKILL.md")).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(f.name, NAME);
        assert!(f.description.ends_with("dclh: say-\"hi\", c:\\x."));
    }

    #[test]
    fn rule_paths() {
        assert_eq!(rule_path(&HOME.join(".yuwanplugins")), "~/.yuwanplugins");
        assert_eq!(rule_path(Path::new(r"Z:\tools\lib")), "//z/tools/lib");
    }

    #[test]
    fn rules_are_added_and_removed_without_leftovers() {
        let rules = vec!["Read(~/a/**)".to_string()];
        let mut data: Map<String, Value> = serde_json::from_str(r#"{"env": {}, "theme": "dark"}"#).unwrap();
        let original = data.clone();
        assert!(edit_rules(&mut data, &rules, true).unwrap());
        assert!(!edit_rules(&mut data, &rules, true).unwrap());
        assert_eq!(data["permissions"]["allow"], json!(["Read(~/a/**)"]));
        assert!(edit_rules(&mut data, &rules, false).unwrap());
        assert_eq!(data, original);
        assert_eq!(data.keys().collect::<Vec<_>>(), ["env", "theme"]); // 键的顺序没变
        let mut kept: Map<String, Value> = serde_json::from_str(r#"{"permissions": {"allow": ["Bash(ls)"], "deny": []}}"#).unwrap();
        edit_rules(&mut kept, &rules, true).unwrap();
        edit_rules(&mut kept, &rules, false).unwrap();
        assert_eq!(kept["permissions"]["allow"], json!(["Bash(ls)"]));
    }
}
