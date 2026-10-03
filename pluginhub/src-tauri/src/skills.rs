//! 独立的 skill：不属于任何插件、单独放在 app 的 skills 文件夹里的 skill。
//!
//! Claude Code 读 ~/.claude/skills/<名字>（这里带 .claude-plugin 的是插件，不算），
//! Codex 读 ~/.codex/skills/<名字>，系统自带的在 ~/.codex/skills/.system 下。
//! 插件中心管理的 skill 只在 ~/.yuwanplugins/skills/<名字> 存一份，两边都用目录链接指过来。
//! 两边都没有单个 skill 的开关：放进来就生效，拆掉链接就等于关掉。
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde_json::{json, Value};

use crate::bail;
use crate::gitx::backup;
use crate::store::*;
use crate::usage::{merge_projects, project_rows, Usage};
use crate::platform;
use crate::util::*;

/// 插件中心管理的 skill 放在这里，每个一个文件夹
pub static SKILLS_DIR: LazyLock<PathBuf> = LazyLock::new(|| PLUGINS_DIR.join("skills"));

/// 这个 app 读独立 skill 的文件夹
pub fn app_dir(app: &str) -> PathBuf {
    if app == "claude" {
        CLAUDE_SKILLS.clone()
    } else {
        CODEX_HOME.join("skills")
    }
}

// ---------------------------------------------------------------- SKILL.md 开头的 YAML

#[derive(Default, Clone, Debug)]
pub struct Front {
    pub name: String,
    pub description: String,
    pub version: String,
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// 找收尾的引号：单引号里 '' 表示一个 '，双引号里 \ 是转义
fn closing(s: &str, q: char) -> Option<usize> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (pos, c) = chars[i];
        if q == '"' && c == '\\' {
            i += 2;
            continue;
        }
        if c == q {
            if q == '\'' && chars.get(i + 1).map(|x| x.1) == Some('\'') {
                i += 2;
                continue;
            }
            return Some(pos);
        }
        i += 1;
    }
    None
}

/// 一个键后面的值，可能跨好几行。返回 (值, 额外用掉的行数)。
fn yaml_value(first: &str, rest: &[&str], indent: usize) -> (String, usize) {
    if let Some(q) = first.chars().next().filter(|c| *c == '\'' || *c == '"') {
        let mut s = first[1..].to_string();
        let mut used = 0;
        loop {
            if let Some(pos) = closing(&s, q) {
                let v = &s[..pos];
                let v = if q == '\'' { v.replace("''", "'") } else { v.replace("\\\"", "\"").replace("\\\\", "\\") };
                return (v, used);
            }
            let Some(next) = rest.get(used) else { return (s, used) };
            s.push(' ');
            s.push_str(next.trim());
            used += 1;
        }
    }
    let deeper = |l: &&&str| l.trim().is_empty() || indent_of(l) > indent;
    if first.starts_with('>') || first.starts_with('|') {
        let lines: Vec<&str> = rest.iter().take_while(deeper).map(|l| l.trim()).collect();
        let used = lines.len();
        let sep = if first.starts_with('|') { "\n" } else { " " };
        return (lines.join(sep).trim().to_string(), used);
    }
    if first.is_empty() {
        return (String::new(), 0); // 下面是嵌套的键（比如 metadata:），留给外层逐行读
    }
    // 普通写法：缩进更深、又不像「键: 值」的行是续行
    let cont: Vec<&str> = rest
        .iter()
        .take_while(|l| !l.trim().is_empty() && indent_of(l) > indent && !l.trim_start().contains(": "))
        .map(|l| l.trim())
        .collect();
    let used = cont.len();
    let mut v = first.to_string();
    for c in cont {
        v.push(' ');
        v.push_str(c);
    }
    (v, used)
}

/// 读 SKILL.md 开头的 name、description 和版本号（顶层的 version，或者 metadata 下的 version）
pub fn read_front(md: &Path) -> Option<Front> {
    let text = String::from_utf8_lossy(&fs::read(md).ok()?).replace("\r\n", "\n");
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();
    let body = text.strip_prefix("---\n")?;
    let end = body.find("\n---")?;
    let lines: Vec<&str> = body[..end].lines().collect();
    let mut f = Front::default();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let indent = indent_of(line);
        let Some((key, value)) = line.trim_start().split_once(':') else {
            i += 1;
            continue;
        };
        let (value, used) = yaml_value(value.trim(), &lines[i + 1..], indent);
        match (indent, key.trim()) {
            (0, "name") => f.name = value,
            (0, "description") => f.description = value,
            (0, "version") => f.version = value,
            (_, "version") if f.version.is_empty() => f.version = value,
            _ => {}
        }
        i += 1 + used;
    }
    Some(f)
}

/// 一个技能每次会话常驻的开销（粗估 token）：名字加说明。按 Claude Code 报的数校准过：
/// 中文大约一字一个半 token，其余大约四个字符一个，每条再加一点格式。
pub fn tokens(name: &str, description: &str) -> u64 {
    let text = format!("{name}: {description}");
    let cjk = text.chars().filter(|c| *c as u32 >= 0x2E80).count() as u64;
    let other = text.chars().count() as u64 - cjk;
    cjk * 3 / 2 + other / 4 + 8
}

// ---------------------------------------------------------------- 扫描两边的 skills 文件夹

/// 在某个 app 里找到的一个独立 skill
#[derive(Clone, Debug)]
pub struct Found {
    pub app: &'static str,
    /// 在 app 的 skills 文件夹里的名字
    pub dir: String,
    /// app 那边的路径，可能是链接
    pub path: PathBuf,
    /// 实际的文件夹：是链接就是它指向的地方
    pub target: PathBuf,
    pub linked: bool,
    /// Codex 自带的（.system 下）
    pub official: bool,
    pub front: Front,
}

fn scan_dir(app: &'static str, dir: &Path, official: bool, out: &mut Vec<Found>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = file_name(&path);
        // .system 另外扫；Claude Code 的 skills 文件夹里也放插件（带 .claude-plugin 的），那些不算
        if name.starts_with('.') || !path.is_dir() || path.join(".claude-plugin").join("plugin.json").is_file() {
            continue;
        }
        let Some(mut front) = read_front(&path.join("SKILL.md")) else { continue };
        if front.name.is_empty() {
            front.name = name.clone();
        }
        let linked = platform::is_link(&path);
        let target = if linked { platform::link_target(&path).unwrap_or_else(|| path.clone()) } else { path.clone() };
        out.push(Found { app, dir: name, path, target, linked, official, front });
    }
}

pub fn scan() -> Vec<Found> {
    let mut out = Vec::new();
    scan_dir("claude", &app_dir("claude"), false, &mut out);
    scan_dir("codex", &app_dir("codex"), false, &mut out);
    scan_dir("codex", &app_dir("codex").join(".system"), true, &mut out);
    out
}

fn backup_name(dir: &str) -> String {
    format!("skill-{dir}")
}

// ---------------------------------------------------------------- 页面上的一行

/// 用过这个 skill 的项目：Claude Code 按 skill 名记，Codex 按文件夹名记，两个名字不一样时都算上
fn skill_projects(usage: &Usage, app: &str, name: &str, dir: &str) -> Vec<Value> {
    let a = format!("skill:{name}");
    let b = format!("skill:{dir}");
    let groups = if a == b { vec![usage.get(app, &a)] } else { vec![usage.get(app, &a), usage.get(app, &b)] };
    project_rows(&merge_projects(&groups))
}

/// 本机所有独立的 skill，同名的合成一行。行的格式和插件一样，页面上的卡片、详情页、按钮都能共用。
pub fn skill_rows(usage: &Usage) -> Vec<Value> {
    let cfg = load_config();
    let found = scan();
    let mut groups: BTreeMap<String, Vec<&Found>> = BTreeMap::new();
    for f in &found {
        groups.entry(f.front.name.clone()).or_default().push(f);
    }
    // 插件中心管的 skill，链接全被删了也要列出来，等同步或后台检查修好
    for s in &cfg.skills {
        groups.entry(s.name.clone()).or_default();
    }
    let mut rows = Vec::new();
    for (name, list) in groups {
        let managed = cfg.skills.iter().find(|s| s.name == name);
        let ours = managed.map(|s| SKILLS_DIR.join(&s.dir)).filter(|d| d.join("SKILL.md").is_file());
        let dir = managed.map(|s| s.dir.clone()).or_else(|| list.first().map(|f| f.dir.clone())).unwrap_or_else(|| name.clone());
        let front = ours
            .as_ref()
            .and_then(|d| read_front(&d.join("SKILL.md")))
            .or_else(|| list.first().map(|f| f.front.clone()))
            .unwrap_or_default();
        let mut apps = Vec::new();
        for app in APPS {
            let here = list.iter().find(|f| f.app == app);
            let wanted = managed.is_some_and(|s| s.targets.get(app).copied().unwrap_or(false));
            if here.is_none() && !(ours.is_some() && wanted) {
                continue;
            }
            let path = here.map(|f| f.path.clone()).unwrap_or_else(|| app_dir(app).join(&dir));
            let mut a = json!({
                "app": app, "id": format!("skill:{name}"), "installed": here.is_some(),
                "version": if front.version.is_empty() { Value::Null } else { Value::from(front.version.clone()) },
                "enabled": here.is_some(), "description": front.description, "path": display(&path),
                "target": here.map(|f| display(&f.target)), "linked": here.map(|f| f.linked).unwrap_or(false),
                "official": here.map(|f| f.official).unwrap_or(false), "port": null,
                "projects": skill_projects(usage, app, &name, here.map(|f| f.dir.as_str()).unwrap_or(&dir)),
            });
            a["source"] = Value::from(match here {
                Some(f) if f.official => "Codex 自带".to_string(),
                Some(f) if f.linked && ours.as_ref().is_some_and(|o| same_path(Some(&f.target), Some(o))) => format!("链接到 {}", display(&f.target)),
                Some(f) if f.linked => format!("链接到 {}", display(&f.target)),
                Some(_) => "独立的文件夹".to_string(),
                None => "—".to_string(),
            });
            if let Some(o) = &ours {
                // 插件中心管的那一份：带 state，页面上按受管的样子显示状态和问题
                let (state, problem) = match here {
                    None => ("missing", format!("{} 这个链接不见了", display(&path))),
                    Some(f) if links_to(&f.path, o) => ("ok", String::new()),
                    Some(f) if f.linked => ("unlinked", format!("{} 指向了别处", display(&f.path))),
                    Some(f) => ("unlinked", format!("{} 是个真实的文件夹，不是链接", display(&f.path))),
                };
                a["state"] = Value::from(state);
                a["problems"] = json!(if problem.is_empty() { Vec::new() } else { vec![problem] });
                a["folder"] = Value::from(display(o));
            }
            apps.push(a);
        }
        let official = !apps.is_empty() && apps.iter().all(|a| gb(a, "official"));
        let supports: Vec<&str> = if official { vec!["codex"] } else { APPS.to_vec() };
        let mut row = json!({
            "key": format!("skill:{name}"), "kind": "skill", "name": name, "version": front.version,
            "description": front.description, "managed": null, "repo": "", "link": "",
            "skill": {"ours": ours.is_some(), "folder": ours.as_deref().map(display), "dir": dir},
            "apps": apps, "supports": supports, "official": official,
        });
        let install = install_options(&row, ours.as_deref(), &dir);
        let installable: Vec<String> = install.iter().filter(|(_, o)| o.get("how").map(|h| !h.is_null()).unwrap_or(false)).map(|(k, _)| k.clone()).collect();
        row["install"] = Value::Object(install);
        row["installable"] = json!(installable);
        row["cost"] = Value::from(tokens(&name, &front.description));
        rows.push(row);
    }
    rows.sort_by_key(|r| (!gb(&r["skill"], "ours"), gb(r, "official"), gs(r, "name").to_lowercase()));
    rows
}

/// 每个还没装的 app 能怎么装：link 直接链接到统一存放的那份；move 先把现有的那份挪进统一存放的地方
fn install_options(row: &Value, ours: Option<&Path>, dir: &str) -> serde_json::Map<String, Value> {
    let apps = ga(row, "apps");
    let mut out = serde_json::Map::new();
    for x in APPS {
        if apps.iter().any(|a| gs(a, "app") == x && gb(a, "installed")) {
            continue;
        }
        let spot = app_dir(x).join(dir);
        let v = if gb(row, "official") {
            json!({"how": null, "why": "Codex 自带的 skill，只在 Codex 里用"})
        } else if fs::symlink_metadata(&spot).is_ok() {
            json!({"how": null, "why": format!("{} 已经被别的东西占了", display(&spot))})
        } else if ours.is_some() {
            json!({"how": "link"})
        } else if let Some(src) = apps.iter().find(|a| gb(a, "installed") && !gb(a, "official")) {
            if gb(src, "linked") {
                json!({"how": null, "why": format!("{} 是指向 {} 的链接，插件中心不动别处的文件夹", gs(src, "path"), gs(src, "target"))})
            } else {
                json!({"how": "move", "from": gs(src, "app")})
            }
        } else {
            json!({"how": null, "why": "找不到它的文件"})
        };
        out.insert(x.to_string(), v);
    }
    out
}

// ---------------------------------------------------------------- 操作

fn ours_of(row: &Value) -> Option<PathBuf> {
    row["skill"].get("folder").and_then(Value::as_str).map(PathBuf::from)
}

/// 两个文件夹里的文件一模一样（相对路径和内容都相同）
fn same_tree(a: &Path, b: &Path) -> bool {
    let files = |root: &Path| -> BTreeMap<String, Vec<u8>> {
        walkdir::WalkDir::new(root)
            .into_iter()
            .flatten()
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| {
                let rel = e.path().strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
                Some((rel, fs::read(e.path()).ok()?))
            })
            .collect()
    };
    files(a) == files(b)
}

/// 收编：把 app 里现有的那份挪进 ~/.yuwanplugins/skills 统一存放，原来的位置换成链接。返回统一存放的文件夹。
fn adopt(row: &Value, dir: &str, notes: &mut Vec<String>) -> R<PathBuf> {
    let name = gs(row, "name");
    let copies: Vec<&Value> = ga(row, "apps").iter().filter(|a| gb(a, "installed") && !gb(a, "official")).collect();
    if copies.is_empty() {
        bail!("找不到 {name} 的文件。");
    }
    if let Some(c) = copies.iter().find(|c| gb(c, "linked")) {
        bail!("{} 是指向 {} 的链接，插件中心没有动它。", gs(c, "path"), gs(c, "target"));
    }
    let paths: Vec<PathBuf> = copies.iter().map(|c| PathBuf::from(gs(c, "path"))).collect();
    if paths.len() > 1 && !same_tree(&paths[0], &paths[1]) {
        bail!("两个 app 里各有一份 {name}，内容不一样。先在详情页删掉不要的那份，再安装。");
    }
    let dest = SKILLS_DIR.join(dir);
    if fs::symlink_metadata(&dest).is_ok() {
        bail!("{} 已经存在，没有动它。", display(&dest));
    }
    fs::create_dir_all(&*SKILLS_DIR)?;
    move_path(&paths[0], &dest)?;
    make_link(&paths[0], &dest)?;
    notes.push(format!("{} 挪到 {} 统一存放，原来的位置换成链接", display(&paths[0]), display(&dest)));
    for (c, p) in copies.iter().zip(&paths).skip(1) {
        let kept = backup(p, &backup_name(dir))?;
        make_link(p, &dest)?;
        notes.push(format!("{} 里一样的那份挪进备份 {}，换成链接", app_name(gs(c, "app")), display(&kept)));
    }
    Ok(dest)
}

/// 装到选中的 app：还没统一存放的先收编，再在每个 app 的 skills 文件夹里放一个链接
pub fn install(row: &Value, apps: &[String]) -> R<Vec<String>> {
    let name = gs(row, "name").to_string();
    let dir = gs(&row["skill"], "dir").to_string();
    let mut notes = Vec::new();
    let ours = match ours_of(row) {
        Some(p) => p,
        None => adopt(row, &dir, &mut notes)?,
    };
    let mut cfg = load_config();
    let i = match cfg.skills.iter().position(|s| s.name == name) {
        Some(i) => i,
        None => {
            cfg.skills.push(SkillCfg { name: name.clone(), dir: dir.clone(), targets: BTreeMap::new() });
            cfg.skills.len() - 1
        }
    };
    // 原本就装着它的 app，收编后原位置已经是链接，也记上
    for a in ga(row, "apps").iter().filter(|a| gb(a, "installed") && !gb(a, "official")) {
        cfg.skills[i].targets.insert(gs(a, "app").to_string(), true);
    }
    for app in apps {
        let link = app_dir(app).join(&dir);
        if make_link(&link, &ours)? {
            notes.push(format!("{}：链接 {} → {}", app_name(app), display(&link), display(&ours)));
        }
        cfg.skills[i].targets.insert(app.clone(), true);
    }
    save_config(&cfg)?;
    Ok(tagged(&name, notes))
}

/// 从一个 app 里删掉：链接只拆链接，真实的文件夹挪进备份。插件中心管的，两边都不要了就把统一存放的那份也挪进备份。
pub fn uninstall_app(row: &Value, app: &str) -> R<Vec<String>> {
    let name = gs(row, "name").to_string();
    let Some(a) = ga(row, "apps").iter().find(|a| gs(a, "app") == app && gb(a, "installed")) else {
        bail!("这个 app 里没装 {name}。");
    };
    if gb(a, "official") {
        bail!("{name} 是 Codex 自带的，不能删。");
    }
    let dir = gs(&row["skill"], "dir").to_string();
    let path = PathBuf::from(gs(a, "path"));
    let mut notes = Vec::new();
    if platform::is_link(&path) {
        platform::remove_link(&path)?;
        notes.push(format!("{}：去掉了链接 {}", app_name(app), display(&path)));
    } else if path.exists() {
        let kept = backup(&path, &backup_name(&dir))?;
        notes.push(format!("{}：{} 挪进了备份 {}", app_name(app), display(&path), display(&kept)));
    }
    let mut cfg = load_config();
    if let Some(i) = cfg.skills.iter().position(|s| s.name == name) {
        cfg.skills[i].targets.insert(app.to_string(), false);
        if !cfg.skills[i].targets.values().any(|v| *v) {
            notes.extend(retire(cfg.skills.remove(i))?);
        }
        save_config(&cfg)?;
    }
    Ok(tagged(&name, notes))
}

/// 从所有 app 删掉（Codex 自带的不动），插件中心也不再管它
pub fn uninstall_all(row: &Value) -> R<Vec<String>> {
    let name = gs(row, "name").to_string();
    let mut notes = Vec::new();
    for a in ga(row, "apps").iter().filter(|a| gb(a, "installed") && !gb(a, "official")) {
        notes.extend(uninstall_app(&find_skill(&name)?, gs(a, "app"))?);
    }
    let mut cfg = load_config();
    if let Some(i) = cfg.skills.iter().position(|s| s.name == name) {
        let s = cfg.skills.remove(i);
        save_config(&cfg)?;
        notes.extend(tagged(&name, retire(s)?));
    }
    Ok(notes)
}

/// 不再管理：统一存放的那份挪进备份，不直接删
fn retire(s: SkillCfg) -> R<Vec<String>> {
    let folder = SKILLS_DIR.join(&s.dir);
    if !folder.exists() {
        return Ok(Vec::new());
    }
    let kept = backup(&folder, &backup_name(&s.dir))?;
    Ok(vec![format!("两边都不用了，插件中心不再管它，{} 挪进了备份 {}", display(&folder), display(&kept))])
}

/// 同步：插件中心管的，把该有的链接都补上
pub fn sync(row: &Value) -> R<Vec<String>> {
    let name = gs(row, "name").to_string();
    let Some(ours) = ours_of(row) else {
        bail!("{name} 还不归插件中心管，没有可同步的。点安装把它装到另一个 app，就会统一存放。");
    };
    let cfg = load_config();
    let Some(s) = cfg.skills.iter().find(|s| s.name == name) else { return Ok(Vec::new()) };
    let mut notes = Vec::new();
    for (app, on) in &s.targets {
        if *on {
            let link = app_dir(app).join(&s.dir);
            if make_link(&link, &ours)? {
                notes.push(format!("{}：重新链接 {} → {}", app_name(app), display(&link), display(&ours)));
            }
        }
    }
    Ok(tagged(&name, notes))
}

/// 后台检查：插件中心管的 skill，每个该有的链接是不是还在、还指向统一存放的那份。返回 (app, 名字, 问题, 修好没有)。
pub fn guard() -> Vec<(String, String, Vec<String>, bool)> {
    let cfg = load_config();
    let mut out = Vec::new();
    for s in &cfg.skills {
        let ours = SKILLS_DIR.join(&s.dir);
        if !ours.join("SKILL.md").is_file() {
            continue;
        }
        for (app, on) in &s.targets {
            let link = app_dir(app).join(&s.dir);
            if !*on || links_to(&link, &ours) {
                continue;
            }
            let mut problems = vec![if platform::is_link(&link) {
                format!("{} 指向了别处", display(&link))
            } else if link.exists() {
                format!("{} 是个真实的文件夹，不是链接", display(&link))
            } else {
                format!("{} 这个链接不见了", display(&link))
            }];
            let ok = match make_link(&link, &ours) {
                Ok(_) => true,
                Err(e) => {
                    problems.push(format!("修复失败：{e}"));
                    false
                }
            };
            out.push((app.clone(), s.name.clone(), problems, ok));
        }
    }
    out
}

/// Claude Code 这边的检查：它没有列出独立 skill 的命令，只能核对链接和 SKILL.md
pub fn check_claude(a: &Value, root: Option<&Path>) -> (bool, String) {
    let path = PathBuf::from(gs(a, "path"));
    if let Some(root) = root {
        if !links_to(&path, root) {
            return (false, format!("{} 不是指向 {} 的链接", display(&path), display(root)));
        }
    }
    match read_front(&path.join("SKILL.md")) {
        None => (false, "SKILL.md 读不了，或者开头没有 --- 包起来的说明".into()),
        Some(f) if f.name.is_empty() || f.description.is_empty() => (false, "SKILL.md 开头缺 name 或 description，Claude Code 认不出来".into()),
        Some(_) => (true, "文件都在，SKILL.md 格式没问题（Claude Code 没有列出技能的命令，只能核对文件）".into()),
    }
}

pub fn find_skill(name: &str) -> R<Value> {
    crate::ops::find_row(&format!("skill:{name}"))
}

fn tagged(name: &str, notes: Vec<String>) -> Vec<String> {
    let notes: Vec<String> = notes.into_iter().map(|n| format!("[{name}] {n}")).collect();
    for n in &notes {
        log(n);
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front(text: &str) -> Front {
        let dir = std::env::temp_dir().join(format!("pluginhub-front-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let md = dir.join("SKILL.md");
        fs::write(&md, text).unwrap();
        let f = read_front(&md).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        f
    }

    #[test]
    fn reads_plain_and_quoted_values() {
        let f = front("---\nname: autoresearch\ndescription: 'It''s a loop. USE FOR: x'\nmetadata:\n  version: \"1.2\"\n---\nbody");
        assert_eq!(f.name, "autoresearch");
        assert_eq!(f.description, "It's a loop. USE FOR: x");
        assert_eq!(f.version, "1.2");
    }

    #[test]
    fn reads_block_and_continued_values() {
        let f = front("---\nname: a\ndescription: >\n  first line\n  second line\nversion: 3\n---\n");
        assert_eq!(f.description, "first line second line");
        assert_eq!(f.version, "3");
        let g = front("---\r\nname: b\r\ndescription: plain start\r\n  continues here\r\n---\r\n");
        assert_eq!(g.description, "plain start continues here");
    }

    #[test]
    fn tokens_count_chinese_heavier() {
        assert_eq!(tokens("a", "bcdefgh"), 10); // "a: bcdefgh" 十个字符 → 2，加 8
        assert_eq!(tokens("a", "中文"), 3 + 8); // 两个汉字算 3，"a: " 不到 4 个字符算 0
    }

    #[test]
    fn no_front_matter_is_none() {
        let dir = std::env::temp_dir().join(format!("pluginhub-front-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "# just a title\n").unwrap();
        assert!(read_front(&dir.join("SKILL.md")).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }
}
