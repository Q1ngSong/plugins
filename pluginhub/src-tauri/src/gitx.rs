//! git 与受管插件的克隆：拉取、跟上远端、看本地有没有修改、读插件清单、备份
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::bail;
use crate::store::PluginCfg;
use crate::util::*;

pub fn git_t(args: &[&str], cwd: Option<&Path>, timeout: u64, check: bool) -> R<String> {
    let g = find_git()?;
    let gs = g.to_string_lossy().to_string();
    let mut full = vec!["-c", "core.quotepath=false"];
    full.extend_from_slice(args);
    Ok(run(&gs, &full, cwd, timeout, check, false)?.out)
}

/// 运行 git，失败就报错
pub fn git(args: &[&str], cwd: Option<&Path>) -> R<String> {
    git_t(args, cwd, 180, true)
}

/// 运行 git，失败也只返回它的输出（可能是空的）
pub fn git_soft(args: &[&str], cwd: Option<&Path>) -> String {
    git_t(args, cwd, 180, false).unwrap_or_default()
}


pub fn repo_dir(p: &PluginCfg) -> PathBuf {
    let d = PLUGINS_DIR.join(&p.id);
    let old = LEGACY_REPOS_DIR.join(&p.id);
    if !d.exists() && old.join(".git").exists() {
        // 旧版本的克隆挪到新位置，两边链接过来后才生效
        if move_path(&old, &d).is_ok() {
            log(&format!("[{}] 克隆从 {} 挪到 {}", p.id, display(&old), display(&d)));
        }
    }
    d
}

pub fn repo_web_url(repo: &str) -> String {
    let re = Regex::new(r"^git@([^:]+):(.+?)(?:\.git)?$").unwrap();
    if let Some(m) = re.captures(repo) {
        return format!("https://{}/{}", &m[1], &m[2]);
    }
    repo.strip_suffix(".git").unwrap_or(repo).to_string()
}

pub fn repo_slug(repo: &str) -> String {
    let re = Regex::new(r"github\.com[:/]+(.+?)(?:\.git)?/?$").unwrap();
    re.captures(repo).map(|m| m[1].to_string()).unwrap_or_else(|| repo.to_string())
}

#[derive(Serialize, Clone, Debug)]
pub struct Commit {
    pub sha: String,
    pub short: String,
    pub subject: String,
    pub date: String,
}

pub fn commit_info(repo: &Path, rev: &str) -> Option<Commit> {
    let out = git_soft(&["log", "-1", "--format=%H%x1f%h%x1f%s%x1f%cI", rev, "--"], Some(repo));
    let parts: Vec<&str> = out.trim().split('\x1f').collect();
    if parts.len() != 4 {
        return None;
    }
    Some(Commit { sha: parts[0].to_string(), short: parts[1].to_string(), subject: parts[2].to_string(), date: parts[3].to_string() })
}

pub fn head_sha(repo: &Path) -> Option<String> {
    if repo.join(".git").exists() {
        commit_info(repo, "HEAD").map(|c| c.sha)
    } else {
        None
    }
}

pub fn resolve_commit(repo: &Path, rev: &str) -> Option<String> {
    let out = git_soft(&["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")], Some(repo));
    let s = out.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

pub fn ensure_clone(p: &PluginCfg) -> R<PathBuf> {
    let d = repo_dir(p);
    if d.join(".git").exists() {
        return Ok(d);
    }
    fs::create_dir_all(&*PLUGINS_DIR)?;
    log(&format!("[{}] 克隆 {}（{} 分支）", p.id, p.repo, p.branch));
    // 保持仓库里的换行符原样：插件里的 bash 钩子遇到 CRLF 会跑不起来。longpaths 只对 Windows 有意义，别的系统 git 会忽略
    let ds = display(&d);
    git_t(
        &["-c", "core.autocrlf=false", "-c", "core.longpaths=true", "clone", "--branch", &p.branch, &p.repo, &ds],
        None,
        900,
        true,
    )?;
    git(&["config", "core.autocrlf", "false"], Some(&d))?;
    git(&["config", "core.longpaths", "true"], Some(&d))?;
    Ok(d)
}

pub fn fetch(p: &PluginCfg) -> R<PathBuf> {
    let d = ensure_clone(p)?;
    git_t(&["fetch", "--prune", "origin"], Some(&d), 600, true)?;
    Ok(d)
}

/// 插件文件夹里的修改：没提交的改动、本地提交、切到了别的分支。没有修改返回空列表。
///
/// synced 是上次同步到的提交。远端分支到过的位置和它都不算本地提交，上游改写历史不会被误判。
pub fn local_changes(d: &Path, branch: &str, synced: Option<&str>) -> Vec<String> {
    let mut found = Vec::new();
    let changed = git_soft(&["status", "--porcelain"], Some(d)).lines().filter(|x| !x.trim().is_empty()).count();
    if changed > 0 {
        found.push(format!("{changed} 个文件有改动"));
    }
    let seen = git_soft(&["reflog", "show", "--format=%H", &format!("refs/remotes/origin/{branch}")], Some(d));
    let mut known: Vec<String> = vec![format!("origin/{branch}")];
    if let Some(s) = synced {
        known.push(s.to_string());
    }
    known.extend(seen.split_whitespace().take(200).map(|s| s.to_string()));
    let mut args: Vec<&str> = vec!["rev-list", "--count", "HEAD", "--not"];
    args.extend(known.iter().map(String::as_str));
    let ahead = git_soft(&args, Some(d));
    if let Ok(n) = ahead.trim().parse::<u64>() {
        if n > 0 {
            found.push(format!("{n} 个本地提交"));
        }
    }
    let current = git_soft(&["rev-parse", "--abbrev-ref", "HEAD"], Some(d)).trim().to_string();
    if !current.is_empty() && current != branch {
        found.push(format!("切到了 {current}"));
    }
    found
}

pub fn modified_hint(d: &Path, changes: &[String]) -> String {
    format!(
        "{} 里有修改（{}），这次没有拉取远端的新版本。不建议在 ~/.yuwanplugins 里改插件：要保留这些修改，就锁定这个插件（不再同步）；不需要的话，另存一份再还原成仓库里的版本。",
        display(d),
        changes.join("、")
    )
}

/// 把克隆移到跟踪分支的最新提交；上游改写过历史也能跟上。返回 (旧提交, 新提交)。
///
/// 插件文件夹里有修改时不动它，返回 LocalChanges，提示锁定或另存。
pub fn sync_clone(p: &PluginCfg, synced: Option<&str>) -> R<(Option<String>, String)> {
    let d = repo_dir(p);
    let old = head_sha(&d);
    fetch(p)?;
    let target = format!("origin/{}", p.branch);
    let Some(new) = resolve_commit(&d, &target) else { bail!("远端没有 {} 分支。", p.branch) };
    let current = git(&["rev-parse", "--abbrev-ref", "HEAD"], Some(&d))?.trim().to_string();
    if old.as_deref() != Some(new.as_str()) || current != p.branch {
        let changes = local_changes(&d, &p.branch, synced);
        if !changes.is_empty() {
            return Err(HubError::LocalChanges(modified_hint(&d, &changes)));
        }
        git(&["checkout", "-B", &p.branch, &target], Some(&d))?;
    }
    Ok((old, new))
}

pub fn remote_branches(d: &Path) -> Vec<String> {
    let out = git_soft(&["for-each-ref", "--format=%(refname:short)", "refs/remotes/origin"], Some(d));
    let mut names: Vec<String> = out
        .split_whitespace()
        .filter(|x| x.contains('/') && !x.ends_with("/HEAD"))
        .map(|x| x.split_once('/').map(|(_, name)| name).unwrap_or("").to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

pub fn default_branch(repo: &str) -> R<String> {
    let out = git_t(&["ls-remote", "--symref", repo, "HEAD"], None, 90, true)?;
    let re = Regex::new(r"ref: refs/heads/(\S+)\s+HEAD").unwrap();
    Ok(re.captures(&out).map(|m| m[1].to_string()).unwrap_or_else(|| "main".to_string()))
}

#[derive(Serialize, Clone, Debug)]
pub struct ClaudePlugin {
    pub name: String,
    pub path: String,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Manifest {
    pub title: String,
    pub description: String,
    pub version: String,
    pub claude_plugins: Vec<ClaudePlugin>,
    pub codex_name: Option<String>,
}

/// 读插件清单。rev 为空读工作目录，否则读仓库里这个提交的文件。
pub fn read_manifests(d: &Path, rev: Option<&str>) -> Manifest {
    let load = |rel: &str| -> Option<Value> {
        match rev {
            None => read_json(&d.join(rel)),
            Some(r) => {
                let out = git_t(&["show", &format!("{r}:{rel}")], Some(d), 180, false).ok()?;
                serde_json::from_str(out.trim_start_matches('\u{feff}')).ok()
            }
        }
    };
    let mk = load(".claude-plugin/marketplace.json");
    let claude_pj = load(".claude-plugin/plugin.json");
    let codex_pj = load(CODEX_MANIFEST);
    let mut info = Manifest::default();
    // Claude Code 的插件：插件源里列出的本地子目录；没有插件源就看根目录的 plugin.json
    if let Some(mk) = mk.as_ref().filter(|v| v.is_object()) {
        for x in ga(mk, "plugins") {
            let name = gs(x, "name");
            let src = x.get("source").and_then(Value::as_str).unwrap_or("");
            if !name.is_empty() && src.starts_with("./") && !src.contains("..") {
                let path = src[2..].trim_matches('/');
                info.claude_plugins.push(ClaudePlugin { name: name.to_string(), path: if path.is_empty() { ".".into() } else { path.to_string() } });
            }
        }
    }
    if info.claude_plugins.is_empty() {
        if let Some(pj) = claude_pj.as_ref().filter(|v| v.is_object()) {
            if !gs(pj, "name").is_empty() {
                info.claude_plugins.push(ClaudePlugin { name: gs(pj, "name").to_string(), path: ".".into() });
            }
        }
    }
    if let Some(pj) = codex_pj.as_ref().filter(|v| v.is_object()) {
        if !gs(pj, "name").is_empty() {
            info.codex_name = Some(gs(pj, "name").to_string());
        }
    }
    for src in [&claude_pj, &codex_pj].into_iter().flatten().filter(|v| v.is_object()) {
        if !gs(src, "name").is_empty() {
            info.title = gs(src, "name").to_string();
        }
        if info.description.is_empty() {
            info.description = gs(src, "description").to_string();
        }
        if info.version.is_empty() {
            let v = match src.get("version") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            };
            info.version = v.split('+').next().unwrap_or("").to_string();
        }
    }
    if info.version.is_empty() {
        if let Some(mk) = mk.as_ref().filter(|v| v.is_object()) {
            info.version = ga(mk, "plugins")
                .iter()
                .find_map(|x| match x.get("version") {
                    Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
                    Some(Value::Number(n)) => Some(n.to_string()),
                    _ => None,
                })
                .unwrap_or_default();
        }
    }
    info
}

/// 这个路径是不是在插件中心自己的插件文件夹里（新旧位置都算）
pub fn ours(path: Option<&Path>) -> bool {
    let Some(path) = path.filter(|p| !p.as_os_str().is_empty()) else { return false };
    [&*PLUGINS_DIR, &*LEGACY_REPOS_DIR].iter().any(|root| under(path, root))
}

pub fn backup(path: &Path, name: &str) -> R<PathBuf> {
    fs::create_dir_all(&*BACKUP_DIR)?;
    let base = format!("{name}-{}", now().format("%Y%m%d-%H%M%S"));
    let mut dest = BACKUP_DIR.join(&base);
    let mut n = 1;
    while dest.exists() {
        // 同一秒备份两次时加序号，不然会挪进前一个里面
        n += 1;
        dest = BACKUP_DIR.join(format!("{base}-{n}"));
    }
    move_path(path, &dest)?;
    prune_backups(name);
    Ok(dest)
}

pub fn prune_backups(name: &str) {
    let prefix = format!("{name}-");
    let Ok(rd) = fs::read_dir(&*BACKUP_DIR) else { return };
    let mut olds: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| file_name(p).starts_with(&prefix)).collect();
    olds.sort_by(|a, b| mtime(b).partial_cmp(&mtime(a)).unwrap_or(std::cmp::Ordering::Equal));
    for old in olds.into_iter().skip(KEEP_BACKUPS) {
        let _ = rmtree(&old);
    }
}
