//! Codex：受管插件在个人插件源里指向 ~/.yuwanplugins/<名字>，Codex 缓存里的子文件夹链接回那里
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::claude::split_id;
use crate::gitx::{backup, ours};
use crate::skills::read_front;
use crate::util::*;

pub fn codex_version(folder: &Path) -> String {
    let m = read_obj(&folder.join(CODEX_MANIFEST));
    match m.get("version") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => "0.0.0".into(),
    }
}

fn read_bytes(p: &Path) -> Option<Vec<u8>> {
    fs::read(p).ok()
}

/// 让 Codex 缓存跟着插件文件夹走：子文件夹做成链接，顶层文件和清单复制一份。返回链接有没有变。
///
/// Codex 不认整个链接过来的缓存文件夹，但会跟着里面的子文件夹链接去读文件，
/// 所以 skill、脚本、钩子的改动不用重装就能生效。
pub fn mirror(cache: &Path, folder: &Path) -> R<bool> {
    let mut changed = false;
    let mut names: Vec<String> = fs::read_dir(folder)?.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n != ".git").collect();
    names.sort();
    if let Ok(rd) = fs::read_dir(cache) {
        for p in rd.flatten() {
            if !names.contains(&p.file_name().to_string_lossy().to_string()) {
                remove_path(&p.path())?;
            }
        }
    }
    for n in &names {
        let (src, dst) = (folder.join(n), cache.join(n));
        if src.is_dir() && n != ".codex-plugin" {
            if !links_to(&dst, &src) {
                remove_path(&dst)?;
                make_junction(&dst, &src)?;
                changed = true;
            }
        } else if src.is_dir() {
            copy_dir(&src, &dst, &[])?;
        } else if !dst.is_file() || read_bytes(&dst) != read_bytes(&src) {
            remove_path(&dst)?;
            fs::copy(&src, &dst)?;
        }
    }
    Ok(changed)
}

/// 模型提示里列出的一个技能：- <名字>: <说明> (file: <SKILL.md 路径>)。插件里的技能，名字写成「插件:技能」
struct PromptSkill {
    name: String,
    description: String,
    file: String,
}

/// 模型提示里的技能清单。清单放不下时，Codex 把路径缩写成 r0/… 这样，前面另附一张对照表（- `r0` = `<文件夹>`），
/// 这里换回完整的路径
fn prompt_skills(text: &str) -> Vec<PromptSkill> {
    // 只看技能那一段，别处碰巧长得像的行不算
    let text = match (text.find("<skills_instructions>"), text.find("</skills_instructions>")) {
        (Some(a), Some(b)) if a < b => &text[a..b],
        _ => text,
    };
    let roots: HashMap<String, String> =
        Regex::new(r"(?m)^- `(r\d+)` = `(.+)`\s*$").unwrap().captures_iter(text).map(|m| (m[1].to_string(), m[2].to_string())).collect();
    Regex::new(r"(?m)^- (\S+): ?(.*)\(file: (.+)\)\s*$")
        .unwrap()
        .captures_iter(text)
        .map(|m| {
            let file = m[3].trim();
            let file = match file.split_once('/') {
                Some((alias, rest)) if roots.contains_key(alias) => format!("{}/{rest}", roots[alias]),
                _ => file.to_string(),
            };
            PromptSkill { name: m[1].to_string(), description: m[2].trim().to_string(), file }
        })
        .collect()
}

/// 清单里有几条说明被 Codex 截短、几条只剩名字：拿提示里的说明和 SKILL.md 原文比
fn trimmed(list: &[PromptSkill]) -> (usize, usize) {
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let (mut cut, mut gone) = (0, 0);
    for s in list {
        let Some(f) = read_front(Path::new(&s.file)) else { continue };
        let (shown, full) = (squash(&s.description), squash(&f.description));
        if full.is_empty() || shown == full {
            continue;
        }
        if shown.is_empty() {
            gone += 1;
        } else if full.starts_with(shown.trim_end_matches(['…', '.'])) {
            cut += 1;
        }
    }
    (cut, gone)
}

/// 技能没出现在清单里时补一句：可能是清单超了上限，排在后面的被拿掉了
fn over_hint(list: &[PromptSkill]) -> &'static str {
    if trimmed(list) == (0, 0) { "" } else { "（Codex 的技能清单超了上限，见首页）" }
}

pub struct Codex {
    pub exe: Option<PathBuf>,
    /// plugin list 和模型提示，同一轮检查里只跑一次（渲染提示要好几秒）
    listed: Option<Value>,
    prompt: Option<String>,
}

impl Codex {
    pub fn new() -> Self {
        Self { exe: find_codex(), listed: None, prompt: None }
    }

    fn exe_str(&self) -> R<String> {
        self.exe.as_ref().map(|p| p.to_string_lossy().to_string()).ok_or_else(|| HubError::Msg("找不到 Codex 的命令行 codex.exe。".into()))
    }

    pub fn cli(&self, args: &[&str]) -> R<String> {
        let r = run(&self.exe_str()?, args, None, 600, true, false)?;
        Ok((r.out + &r.err).trim().to_string())
    }

    /// Codex 渲染出来、真正交给模型的提示（codex debug prompt-input），拼成一段文字
    pub fn prompt_text(&mut self) -> R<&str> {
        if self.prompt.is_none() {
            let out = run(&self.exe_str()?, &["debug", "prompt-input", "hi"], None, 180, true, false)?.out;
            let v: Value = serde_json::from_str(if out.trim().is_empty() { "[]" } else { &out })?;
            let mut texts = Vec::new();
            fn walk(x: &Value, texts: &mut Vec<String>) {
                match x {
                    Value::String(s) => texts.push(s.clone()),
                    Value::Object(m) => m.values().for_each(|v| walk(v, texts)),
                    Value::Array(a) => a.iter().for_each(|v| walk(v, texts)),
                    _ => {}
                }
            }
            walk(&v, &mut texts);
            self.prompt = Some(texts.join("\n"));
        }
        Ok(self.prompt.as_deref().expect("filled"))
    }

    /// 这一轮渲染过模型提示没有
    pub fn rendered(&self) -> bool {
        self.prompt.is_some()
    }

    /// Codex 的技能清单超没超上限。清单的地方有限（大约是模型上下文的 2%），超了 Codex 不报错：
    /// 先把每条说明截短，再只留名字，最后把排在后面的整条拿掉。模型是看着说明挑技能的，所以拿提示里的说明和原文比。
    /// 返回 (没超, 说明)
    pub fn budget(&mut self) -> R<(bool, String)> {
        let list = prompt_skills(self.prompt_text()?);
        let (cut, gone) = trimmed(&list);
        if cut + gone == 0 {
            return Ok((true, format!("列出的 {} 个技能，说明都是全文", list.len())));
        }
        let what: Vec<String> = [(cut, "说明被截短"), (gone, "只剩名字")].iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{n} 个{w}")).collect();
        Ok((false, format!("超了上限，列出的 {} 个技能里 {}", list.len(), what.join("、"))))
    }

    /// 让 Codex 自己说：plugin list 里有没有、启没启用，渲染出的模型提示里看不看得到每个 skill。
    ///
    /// root 是插件中心放插件的文件夹；给了就再确认 Codex 读的正是那里的文件，而不是缓存里的旧副本。
    pub fn verify(&mut self, cid: &str, expect: &[String], root: Option<&Path>) -> R<(bool, String)> {
        let exe = self.exe_str()?;
        if self.listed.is_none() {
            let out = run(&exe, &["plugin", "list", "--json"], None, 120, true, false)?.out;
            self.listed = Some(serde_json::from_str(if out.trim().is_empty() { "{}" } else { &out })?);
        }
        let listed = self.listed.as_ref().expect("filled");
        let e = ga(listed, "installed").iter().find(|x| gs(x, "pluginId") == cid);
        let Some(e) = e.filter(|e| gb(e, "installed")) else {
            return Ok((false, format!("codex plugin list 里没有 {cid}")));
        };
        if !gb(e, "enabled") {
            return Ok((false, format!("codex plugin list 里有 {cid}，但它是停用的")));
        }
        // 模型提示里插件的每个 skill 一行，名字是「插件:skill」
        let list = prompt_skills(self.prompt_text()?);
        let prefix = format!("{}:", split_id(cid).0);
        let seen: Vec<(&str, &str)> = list.iter().filter_map(|s| Some((s.name.strip_prefix(&prefix)?, s.file.as_str()))).collect();
        let missing: Vec<&String> = expect.iter().filter(|s| !seen.iter().any(|(n, _)| n == s)).collect();
        if !missing.is_empty() {
            let shown: Vec<&str> = missing.iter().take(5).map(|s| s.as_str()).collect();
            return Ok((false, format!("模型提示里少了 {} 个 skill：{}{}", missing.len(), shown.join("、"), over_hint(&list))));
        }
        let broken: Vec<&str> = seen.iter().filter(|(_, f)| !Path::new(f).is_file()).map(|(n, _)| *n).collect();
        if !broken.is_empty() {
            let shown: Vec<&str> = broken.iter().take(5).copied().collect();
            return Ok((false, format!("模型提示里有 {} 个 skill 的文件打不开：{}", broken.len(), shown.join("、"))));
        }
        if let Some(root) = root {
            let base = norm(root) + "\\";
            let outside = seen.iter().filter(|(_, f)| !norm(Path::new(f)).starts_with(&base)).count();
            if outside > 0 {
                return Ok((false, format!("Codex 读的不是 {} 里的文件（{outside} 个 skill 读的是缓存里的旧副本）", display(root))));
            }
        }
        Ok((true, format!("codex plugin list 里已启用，模型提示里能看到 {} 个 skill", seen.len())))
    }

    /// 独立的 skill：渲染出的模型提示里有没有它，文件打不打得开。
    ///
    /// root 是插件中心统一存放它的文件夹；给了就再确认 Codex 读的正是那里的文件。
    pub fn verify_skill(&mut self, name: &str, root: Option<&Path>) -> R<(bool, String)> {
        let list = prompt_skills(self.prompt_text()?);
        let Some(s) = list.iter().find(|s| s.name == name) else {
            return Ok((false, format!("Codex 的模型提示里没有 {name}{}", over_hint(&list))));
        };
        let file = &s.file;
        if !Path::new(&file).is_file() {
            return Ok((false, format!("模型提示里 {name} 指向的 {file} 打不开")));
        }
        if let Some(root) = root {
            if !norm(Path::new(&file)).starts_with(&(norm(root) + "\\")) {
                return Ok((false, format!("Codex 读的是 {file}，不是 {} 里的那份", display(root))));
            }
        }
        Ok((true, "模型提示里能看到它，文件也打得开".into()))
    }

    pub fn config() -> toml::Table {
        fs::read_to_string(CODEX_HOME.join("config.toml")).ok().and_then(|t| toml::from_str::<toml::Table>(&t).ok()).unwrap_or_default()
    }

    /// config.toml 里 [plugins."<id>"] 那一段
    pub fn plugin_conf(cid: &str) -> Option<toml::Table> {
        Self::config().get("plugins").and_then(|v| v.as_table()).and_then(|t| t.get(cid)).and_then(|v| v.as_table()).cloned()
    }

    /// config.toml 里登记的所有插件 ID
    pub fn configured() -> Vec<String> {
        let mut ids: Vec<String> = Self::config().get("plugins").and_then(|v| v.as_table()).map(|t| t.keys().cloned().collect()).unwrap_or_default();
        ids.sort();
        ids
    }

    pub fn conf_enabled(conf: &toml::Table) -> bool {
        conf.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true)
    }

    pub fn personal() -> Map<String, Value> {
        read_obj(&PERSONAL_MARKETPLACE)
    }

    pub fn marketplace_name() -> String {
        Self::personal().get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("personal").to_string()
    }

    pub fn entry_path(&self, name: &str) -> Option<PathBuf> {
        let data = Self::personal();
        for e in data.get("plugins").and_then(Value::as_array).into_iter().flatten() {
            if e.is_object() && gs(e, "name") == name {
                let src = e.get("source").cloned().unwrap_or(Value::Null);
                if gs(&src, "source") == "local" && !gs(&src, "path").is_empty() {
                    return Some(std::path::absolute(HOME.join(gs(&src, "path"))).unwrap_or_else(|_| HOME.join(gs(&src, "path"))));
                }
            }
        }
        None
    }

    pub fn cache_root(&self, name: &str, mkt: Option<&str>) -> PathBuf {
        CODEX_HOME.join("plugins").join("cache").join(mkt.map(str::to_string).unwrap_or_else(Self::marketplace_name)).join(name)
    }

    pub fn cache_dir(&self, name: &str, version: &str) -> PathBuf {
        self.cache_root(name, None).join(version)
    }

    pub fn cache_versions(&self, name: &str, mkt: Option<&str>) -> Vec<PathBuf> {
        let root = self.cache_root(name, mkt);
        let Ok(rd) = fs::read_dir(&root) else { return Vec::new() };
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort_by(|a, b| mtime(a).partial_cmp(&mtime(b)).unwrap_or(std::cmp::Ordering::Equal));
        dirs
    }

    /// 先拆掉缓存里的链接，Codex 清理缓存时就碰不到我们的原文件
    pub fn unlink_cache(d: &Path) {
        if let Ok(rd) = fs::read_dir(d) {
            for p in rd.flatten() {
                if is_junction(&p.path()) {
                    let _ = fs::remove_dir(p.path());
                }
            }
        }
    }

    pub fn linked(cache: &Path, folder: &Path) -> bool {
        let Ok(rd) = fs::read_dir(folder) else { return false };
        let dirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !matches!(file_name(p).as_str(), ".git" | ".codex-plugin"))
            .collect();
        cache.is_dir() && dirs.iter().all(|p| links_to(&cache.join(file_name(p)), p))
    }

    pub fn status(&self, name: &str, folder: &Path, head: Option<&str>, latest: Option<&str>) -> Value {
        let cid = format!("{name}@{}", Self::marketplace_name());
        let conf = Self::plugin_conf(&cid);
        let versions = self.cache_versions(name, None);
        let cache = self.cache_dir(name, &codex_version(folder));
        let entry = self.entry_path(name);
        let installed = conf.is_some() && !versions.is_empty();
        let linked = installed && Self::linked(&cache, folder);
        let enabled = installed && conf.as_ref().map(Self::conf_enabled).unwrap_or(false);
        // 具体哪里不对，以及相关配置文件什么时候被改过（别的程序改掉配置时好对上号）
        let config_toml = CODEX_HOME.join("config.toml");
        let mut problems = Vec::new();
        match &conf {
            None => problems.push(format!("config.toml 里没有这个插件（{}）", changed_at(&config_toml))),
            Some(c) if !Self::conf_enabled(c) => problems.push(format!("在 config.toml 里被关掉了（{}）", changed_at(&config_toml))),
            _ => {}
        }
        let entry_ok = entry.as_deref().map(|e| same_path(Some(e), Some(folder))).unwrap_or(false);
        match &entry {
            None => problems.push(format!("个人插件源里没有它（{}）", changed_at(&PERSONAL_MARKETPLACE))),
            Some(e) if !entry_ok => problems.push(format!("个人插件源指向了 {}，不是 {}", display(e), display(folder))),
            _ => {}
        }
        if conf.is_some() && versions.is_empty() {
            problems.push("Codex 缓存里没有它".into());
        } else if !versions.is_empty() && !Self::linked(&cache, folder) {
            problems.push("Codex 缓存里的链接断了，或者被换成了复制的副本".into());
        }
        let state = if !installed {
            "missing"
        } else if !linked || !entry_ok {
            "unlinked"
        } else if !enabled {
            "disabled"
        } else if latest.is_some() && head != latest {
            "outdated"
        } else {
            "ok"
        };
        let version = if linked { Some(file_name(&cache)) } else { versions.last().map(|p| file_name(p)) };
        json!({
            "id": cid, "state": state, "problems": problems, "folder": display(folder),
            "installed": installed, "enabled": enabled, "linked": linked,
            "commit": if linked { head.map(Value::from).unwrap_or(Value::Null) } else { Value::Null },
            "version": version,
            "source": if linked { format!("链接到 {}", display(folder)) } else { entry.as_deref().map(display).unwrap_or_else(|| "—".into()) },
        })
    }

    /// 个人插件源指向插件文件夹；没装或版本号变了就正常装一份，再把缓存里的子文件夹换成链接
    pub fn link(&self, name: &str, folder: &Path, force: bool) -> R<Vec<String>> {
        let mut notes = Vec::new();
        let cid = format!("{name}@{}", Self::marketplace_name());
        if let Some(old) = self.set_entry(name, folder)? {
            if old.exists() && !ours(Some(&old)) {
                notes.push(format!("旧的源文件夹备份到 {}", display(&backup(&old, name)?)));
            }
        }
        let conf = Self::plugin_conf(&cid);
        let cache = self.cache_dir(name, &codex_version(folder));
        if force || !cache.is_dir() || !conf.as_ref().map(Self::conf_enabled).unwrap_or(false) {
            for d in self.cache_versions(name, None) {
                Self::unlink_cache(&d);
            }
            self.cli(&["plugin", "add", &cid, "--json"])?;
            notes.push(format!("安装 {cid}（版本 {}）", file_name(&cache)));
        }
        if mirror(&cache, folder)? {
            notes.push(format!("缓存里的文件夹链接到 {}", display(folder)));
        }
        for d in self.cache_versions(name, None) {
            if !same_path(Some(&d), Some(&cache)) {
                // 旧版本的缓存
                Self::unlink_cache(&d);
                rmtree(&d)?;
            }
        }
        Ok(notes)
    }

    pub fn remove(&self, cid: &str) -> R<()> {
        let (name, mkt) = split_id(cid);
        for d in self.cache_versions(name, Some(mkt)) {
            Self::unlink_cache(&d);
        }
        self.cli(&["plugin", "remove", cid])?;
        Ok(())
    }

    /// 个人插件源里这个插件指向 folder。改动了就返回原来指向的文件夹。
    pub fn set_entry(&self, name: &str, folder: &Path) -> R<Option<PathBuf>> {
        let mut data = Self::personal();
        let rel = format!("./{}", relative_to_home(folder).replace('\\', "/"));
        let old = self.entry_path(name);
        let plugins = data.entry("plugins").or_insert_with(|| Value::Array(Vec::new()));
        if !plugins.is_array() {
            *plugins = Value::Array(Vec::new());
        }
        let list = plugins.as_array_mut().expect("array");
        match list.iter_mut().find(|x| x.is_object() && gs(x, "name") == name) {
            None => {
                list.push(json!({"name": name, "source": {"source": "local", "path": rel},
                                 "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
                                 "category": "Productivity"}));
                data.entry("name").or_insert_with(|| Value::from("personal"));
                data.entry("interface").or_insert_with(|| json!({"displayName": "Personal"}));
            }
            Some(e) => {
                if e.get("source").map(|s| gs(s, "path") == rel).unwrap_or(false) {
                    return Ok(None);
                }
                e["source"] = json!({"source": "local", "path": rel});
            }
        }
        write_json(&PERSONAL_MARKETPLACE, &Value::Object(data))?;
        log(&format!("[{name}] Codex 个人插件源改为指向 {}", display(folder)));
        Ok(old)
    }

    /// 从个人插件源里去掉这个插件；以前复制出来的源文件夹挪进备份，我们自己的文件夹不动
    pub fn forget(&self, name: &str) -> R<()> {
        let old = self.entry_path(name);
        let mut data = Self::personal();
        let plugins = data.get("plugins").and_then(Value::as_array).cloned().unwrap_or_default();
        let kept: Vec<Value> = plugins.iter().filter(|e| !(e.is_object() && gs(e, "name") == name)).cloned().collect();
        if kept.len() != plugins.len() {
            data.insert("plugins".into(), Value::Array(kept));
            write_json(&PERSONAL_MARKETPLACE, &Value::Object(data))?;
        }
        if let Some(old) = old {
            if old.exists() && !ours(Some(&old)) {
                backup(&old, name)?;
            }
        }
        Ok(())
    }
}

/// folder 相对家目录的写法（个人插件源里的 path 是相对家目录的）
fn relative_to_home(folder: &Path) -> String {
    let f = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
    let home = std::path::absolute(&*HOME).unwrap_or_else(|_| HOME.clone());
    match f.strip_prefix(&home) {
        Ok(rel) => rel.to_string_lossy().to_string(),
        Err(_) => {
            // 不在家目录下：按目录层级算相对路径
            let fc: Vec<_> = f.components().collect();
            let hc: Vec<_> = home.components().collect();
            let common = fc.iter().zip(hc.iter()).take_while(|(a, b)| a == b).count();
            let mut parts: Vec<String> = vec!["..".to_string(); hc.len() - common];
            parts.extend(fc[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
            parts.join("\\")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 清单放不下时的样子（照 Codex 0.139 渲染出来的格式）：路径缩写成 r0/…，说明截短，再不够就只剩名字
    const SHORT: &str = "<skills_instructions>\n## Skills\n### Skill roots\n- `r0` = `C:/Users/a/.codex/skills`\n- `r1` = `C:/Users/a/.yuwanplugins/dclh/skills`\n\
### Available skills\n- autoresearch: Autonomous iterative experimentation loop for any programming task (file: r0/autoresearch/SKILL.md)\n\
- dclh:writing-minimal-code: (file: r1/writing-minimal-code/SKILL.md)\n- pdf:pdf: Read PDF files (file: C:/x/pdf/SKILL.md)\n</skills_instructions>\n- not: a skill (file: elsewhere)\n";

    #[test]
    fn short_paths_are_expanded() {
        let list = prompt_skills(SHORT);
        let rows: Vec<(&str, &str, &str)> = list.iter().map(|s| (s.name.as_str(), s.description.as_str(), s.file.as_str())).collect();
        assert_eq!(
            rows,
            [
                ("autoresearch", "Autonomous iterative experimentation loop for any programming task", "C:/Users/a/.codex/skills/autoresearch/SKILL.md"),
                ("dclh:writing-minimal-code", "", "C:/Users/a/.yuwanplugins/dclh/skills/writing-minimal-code/SKILL.md"),
                ("pdf:pdf", "Read PDF files", "C:/x/pdf/SKILL.md"),
            ]
        );
    }

    #[test]
    fn cut_and_dropped_descriptions_are_counted() {
        let dir = std::env::temp_dir().join(format!("pluginhub-budget-{}", uuid::Uuid::new_v4().simple()));
        let skill = |name: &str, desc: &str| {
            let d = dir.join(name);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("SKILL.md"), format!("---\nname: {name}\ndescription: {desc}\n---\n")).unwrap();
            display(&d.join("SKILL.md"))
        };
        let full = "Use before writing or changing any code";
        let list = [("whole", full), ("cut", "Use before writing or"), ("gone", "")]
            .map(|(name, shown)| PromptSkill { name: name.into(), description: shown.into(), file: skill(name, full) });
        let counts = trimmed(&list);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(counts, (1, 1));
        assert_eq!(trimmed(&list), (0, 0)); // 读不到原文的不算
    }
}
