//! Claude Code：受管插件在 ~/.claude/skills/<名字> 放一个链接，Claude Code 就地加载成 <名字>@skills-dir
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::gitx::{backup, ours};
use crate::util::*;

pub fn describe_claude_source(mk: Option<&Value>) -> String {
    let Some(mk) = mk.filter(|v| v.is_object()) else { return "—".into() };
    let src = mk.get("source").cloned().unwrap_or(Value::Null);
    match gs(&src, "source") {
        "github" => format!("GitHub 下载的副本（{}）", gs(&src, "repo")),
        "git" | "url" => format!("Git 下载的副本（{}）", gs(&src, "url")),
        "directory" => format!("本地目录 {}", gs(&src, "path")),
        "" => "—".into(),
        other => other.to_string(),
    }
}

/// 插件 ID「名字@插件源」拆开
pub fn split_id(pid: &str) -> (&str, &str) {
    match pid.split_once('@') {
        Some((a, b)) => (a, b),
        None => (pid, ""),
    }
}

pub struct ClaudeCode {
    pub exe: Option<PathBuf>,
    /// plugin list 的结果，同一轮检查里只跑一次
    listed: Option<Vec<Value>>,
}

impl ClaudeCode {
    pub fn new() -> Self {
        Self { exe: find_claude(), listed: None }
    }

    fn exe_str(&self) -> R<String> {
        self.exe.as_ref().map(|p| p.to_string_lossy().to_string()).ok_or_else(|| HubError::Msg("找不到 Claude Code 的命令行 claude.exe。".into()))
    }

    pub fn cli(&self, args: &[&str]) -> R<String> {
        let r = run(&self.exe_str()?, args, None, 300, true, false)?;
        Ok((r.out + &r.err).trim().to_string())
    }

    /// 让 Claude Code 自己说：plugin list 里有没有、启没启用，plugin details 里认出了哪些 skill。
    ///
    /// root 是插件中心放插件的文件夹；给了就再确认 Claude Code 加载的正是链接到那里的这份。
    pub fn verify(&mut self, pid: &str, expect: &[String], root: Option<&Path>) -> R<(bool, String)> {
        let exe = self.exe_str()?;
        if self.listed.is_none() {
            let out = run(&exe, &["plugin", "list", "--json"], None, 120, true, false)?.out;
            let v: Value = serde_json::from_str(if out.trim().is_empty() { "[]" } else { &out })?;
            self.listed = Some(v.as_array().cloned().unwrap_or_default());
        }
        let listed = self.listed.as_ref().expect("filled");
        let Some(e) = listed.iter().find(|x| x.is_object() && gs(x, "id") == pid) else {
            return Ok((false, format!("claude plugin list 里没有 {pid}")));
        };
        if e.get("enabled") == Some(&Value::Bool(false)) {
            return Ok((false, format!("claude plugin list 里有 {pid}，但它是停用的")));
        }
        if let Some(root) = root {
            let install = expand_user(gs(e, "installPath"));
            if !links_to(&install, root) {
                return Ok((false, format!("Claude Code 加载的是 {}，不是链接到 {} 的那份", gs(e, "installPath"), display(root))));
            }
        }
        let details = run(&exe, &["plugin", "details", pid], None, 120, true, false)?.out;
        let re = Regex::new(r"(?m)^\s*Skills \((\d+)\)[ \t]*(.*)$").unwrap();
        let seen: Vec<String> = re
            .captures(&details)
            .map(|m| m[2].split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        let missing: Vec<&String> = expect.iter().filter(|s| !seen.contains(s)).collect();
        if !missing.is_empty() {
            let shown: Vec<&str> = missing.iter().take(5).map(|s| s.as_str()).collect();
            return Ok((false, format!("claude plugin details 里少了 {} 个 skill：{}", missing.len(), shown.join("、"))));
        }
        Ok((true, format!("claude plugin list 里已启用，plugin details 认出 {} 个 skill", seen.len())))
    }

    /// 按插件源装的插件（会复制进缓存）
    pub fn installed() -> Map<String, Value> {
        let data = read_obj(&CLAUDE_PLUGINS.join("installed_plugins.json"));
        data.get("plugins").and_then(Value::as_object).cloned().unwrap_or_default()
    }

    /// ~/.claude/skills 下就地加载的插件：{插件 ID: 文件夹}
    pub fn skills_dir() -> BTreeMap<String, PathBuf> {
        let mut out = BTreeMap::new();
        if let Ok(rd) = fs::read_dir(&*CLAUDE_SKILLS) {
            for d in rd.flatten() {
                let p = d.path();
                if p.join(".claude-plugin").join("plugin.json").is_file() {
                    out.insert(format!("{}@skills-dir", d.file_name().to_string_lossy()), p);
                }
            }
        }
        out
    }

    pub fn marketplaces() -> Map<String, Value> {
        read_obj(&CLAUDE_PLUGINS.join("known_marketplaces.json"))
    }

    pub fn enabled_map() -> Map<String, Value> {
        let data = read_obj(&CLAUDE_HOME.join("settings.json"));
        data.get("enabledPlugins").and_then(Value::as_object).cloned().unwrap_or_default()
    }

    /// 就地加载的默认启用，只有明确写了 false 才算关掉
    pub fn enabled_unless_false(pid: &str) -> bool {
        Self::enabled_map().get(pid) != Some(&Value::Bool(false))
    }

    pub fn entry(&self, pid: &str) -> Option<Value> {
        let entries = Self::installed().get(pid).and_then(Value::as_array).cloned().unwrap_or_default();
        entries.iter().find(|e| gs(e, "scope") == "user").cloned().or_else(|| entries.first().cloned())
    }

    /// 同名插件按插件源装的副本
    pub fn copies(&self, name: &str) -> Vec<String> {
        Self::installed().keys().filter(|pid| split_id(pid).0 == name).cloned().collect()
    }

    pub fn status(&self, name: &str, folder: &Path, head: Option<&str>, latest: Option<&str>) -> Value {
        let link = CLAUDE_SKILLS.join(name);
        let linked = links_to(&link, folder);
        let copies = self.copies(name);
        let pid = if linked || copies.is_empty() { format!("{name}@skills-dir") } else { copies[0].clone() };
        let installed = linked || !copies.is_empty();
        let enabled_map = Self::enabled_map();
        // 就地加载的默认启用，只有明确写了 false 才算关掉；按插件源装的要明确写 true
        let enabled = if linked { enabled_map.get(&pid) != Some(&Value::Bool(false)) } else { enabled_map.get(&pid) == Some(&Value::Bool(true)) };
        // 具体哪里不对，以及相关配置文件什么时候被改过（别的程序改掉配置时好对上号）
        let mut problems = Vec::new();
        if !linked {
            problems.push(if is_junction(&link) {
                format!("{} 指向了别处", display(&link))
            } else if link.exists() {
                format!("{} 是个真实文件夹，不是链接", display(&link))
            } else {
                format!("{} 这个链接不见了", display(&link))
            });
        }
        if !copies.is_empty() {
            problems.push(format!("还有按插件源装的副本（{}）{}", copies.join("、"), if linked { "，会加载两份" } else { "" }));
        }
        if installed && !enabled {
            problems.push(format!("在 settings.json 里被关掉了（{}）", changed_at(&CLAUDE_HOME.join("settings.json"))));
        }
        let state = if !installed {
            "missing"
        } else if !linked || !copies.is_empty() {
            "unlinked"
        } else if !enabled {
            "disabled"
        } else if latest.is_some() && head != latest {
            "outdated"
        } else {
            "ok"
        };
        let pj = read_obj(&folder.join(".claude-plugin").join("plugin.json"));
        let entry = self.entry(&pid).unwrap_or(Value::Null);
        let source = if linked {
            format!("链接到 {}", display(folder))
        } else {
            describe_claude_source(Self::marketplaces().get(split_id(&pid).1))
        };
        json!({
            "id": pid, "state": state, "problems": problems, "folder": display(folder),
            "installed": installed, "enabled": enabled, "linked": linked,
            "commit": if linked { head.map(Value::from).unwrap_or(Value::Null) } else { Value::Null },
            "version": if linked { pj.get("version").cloned().unwrap_or(Value::Null) } else { entry.get("version").cloned().unwrap_or(Value::Null) },
            "path": if linked { Value::from(display(&link)) } else { entry.get("installPath").cloned().unwrap_or(Value::Null) },
            "source": source,
        })
    }

    /// 把插件链接进 ~/.claude/skills，并去掉以前按插件源装的同名副本，免得加载两份
    pub fn link(&self, name: &str, folder: &Path) -> R<Vec<String>> {
        let mut notes = Vec::new();
        for pid in self.copies(name) {
            if !Self::installed().contains_key(&pid) {
                continue; // 去掉插件源时已经连带卸掉了
            }
            let mkt = split_id(&pid).1.to_string();
            let src = Self::marketplaces().get(&mkt).and_then(|m| m.get("source")).cloned().unwrap_or(Value::Null);
            if matches!(gs(&src, "source"), "directory" | "file") && ours(Some(Path::new(gs(&src, "path")))) {
                self.cli(&["plugin", "marketplace", "remove", &mkt])?;
                notes.push(format!("去掉以前登记的插件源 {mkt}，改为链接"));
            } else {
                self.cli(&["plugin", "uninstall", &pid])?;
                notes.push(format!("卸载按插件源装的副本 {pid}，改为链接"));
            }
        }
        let link = CLAUDE_SKILLS.join(name);
        if make_junction(&link, folder)? {
            notes.push(format!("链接 {} → {}", display(&link), display(folder)));
        }
        let pid = format!("{name}@skills-dir");
        if Self::enabled_map().get(&pid) == Some(&Value::Bool(false)) {
            self.cli(&["plugin", "enable", &pid])?;
            notes.push(format!("{pid} 被关掉了，已重新启用"));
        }
        Ok(notes)
    }

    /// 卸载。就地加载的只去掉链接；如果是真实文件夹就挪进备份，不直接删。
    pub fn remove(&self, pid: &str) -> R<()> {
        let (name, mkt) = split_id(pid);
        if mkt != "skills-dir" {
            let src = Self::marketplaces().get(mkt).and_then(|m| m.get("source")).cloned().unwrap_or(Value::Null);
            if matches!(gs(&src, "source"), "directory" | "file") && ours(Some(Path::new(gs(&src, "path")))) {
                self.cli(&["plugin", "marketplace", "remove", mkt])?; // 以前登记的插件源，连带卸掉副本
            } else {
                self.cli(&["plugin", "uninstall", pid])?;
            }
            return Ok(());
        }
        let d = CLAUDE_SKILLS.join(name);
        if is_junction(&d) {
            fs::remove_dir(&d)?;
        } else if d.exists() {
            backup(&d, name)?;
        }
        Ok(())
    }
}

/// 把开头的 ~ 换成家目录（claude plugin list 里的路径是这么写的）
pub fn expand_user(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        HOME.join(rest)
    } else if p == "~" {
        HOME.clone()
    } else {
        PathBuf::from(p)
    }
}
