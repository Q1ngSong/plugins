//! git 与受管插件的克隆：拉取、跟上远端、看本地有没有修改、读插件清单、备份
use std::collections::BTreeMap;
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

/// 传输中途被掐断时常见的报错（代理或线路不稳）。这类错再试一次通常就好；地址不对、没权限这些不算。
fn transient(msg: &str) -> bool {
    const SIGNS: &[&str] = &["curl 18", "curl 56", "curl 92", "early EOF", "unexpected disconnect", "Connection reset", "HTTP/2 stream", "SSL_ERROR_SYSCALL"];
    SIGNS.iter().any(|x| msg.contains(x))
}

/// 要联网的 git 操作（clone、fetch、ls-remote）。传输中途被掐断就再试两次，最后一次改用 HTTP/1.1。
/// partial 是这次克隆新建的目标文件夹，重试前把半截的删掉；目标原来就在的不要传，免得删到别的东西。
pub fn git_net(args: &[&str], cwd: Option<&Path>, timeout: u64, partial: Option<&Path>) -> R<String> {
    let mut attempt = 0;
    loop {
        let mut full: Vec<&str> = if attempt == 2 { vec!["-c", "http.version=HTTP/1.1"] } else { Vec::new() };
        full.extend_from_slice(args);
        match git_t(&full, cwd, timeout, true) {
            Ok(out) => return Ok(out),
            Err(e) if attempt < 2 && transient(&e.to_string()) => {
                attempt += 1;
                log(&format!("git {} 传输中断，重试第 {attempt} 次", args.first().copied().unwrap_or("")));
                if let Some(d) = partial {
                    let _ = rmtree(d);
                }
            }
            Err(e) => return Err(e),
        }
    }
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
    let fresh = !d.exists();
    git_net(
        &["-c", "core.autocrlf=false", "-c", "core.longpaths=true", "clone", "--branch", &p.branch, &p.repo, &ds],
        None,
        900,
        fresh.then_some(d.as_path()),
    )?;
    git(&["config", "core.autocrlf", "false"], Some(&d))?;
    git(&["config", "core.longpaths", "true"], Some(&d))?;
    Ok(d)
}

pub fn fetch(p: &PluginCfg) -> R<PathBuf> {
    let d = ensure_clone(p)?;
    git_net(&["fetch", "--prune", "origin"], Some(&d), 600, None)?;
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

/// 远端的默认分支和全部分支名（一次 ls-remote）
pub fn remote_heads(repo: &str) -> R<(String, Vec<String>)> {
    let out = git_net(&["ls-remote", "--symref", repo, "HEAD", "refs/heads/*"], None, 90, None)?;
    let mut default = String::new();
    let mut heads = Vec::new();
    for line in out.lines() {
        let Some((left, right)) = line.split_once('\t') else { continue };
        if let Some(name) = left.strip_prefix("ref: refs/heads/") {
            if right.trim() == "HEAD" {
                default = name.to_string();
            }
        } else if let Some(name) = right.trim().strip_prefix("refs/heads/") {
            heads.push(name.to_string());
        }
    }
    if default.is_empty() {
        default = heads.iter().find(|h| *h == "main" || *h == "master").or(heads.first()).cloned().unwrap_or_else(|| "main".to_string());
    }
    Ok((default, heads))
}

pub fn default_branch(repo: &str) -> R<String> {
    Ok(remote_heads(repo)?.0)
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

impl Manifest {
    pub fn is_plugin(&self) -> bool {
        !self.claude_plugins.is_empty() || self.codex_name.is_some()
    }
}

/// 一个提交里的全部文件：路径 → 文件内容的哈希
pub type Tree = BTreeMap<String, String>;

pub fn list_tree(d: &Path, rev: &str) -> Tree {
    git_soft(&["ls-tree", "-r", rev], Some(d))
        .lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            let mut it = meta.split_whitespace().skip(1); // 权限 类型 哈希
            let (kind, sha) = (it.next()?, it.next()?);
            (kind == "blob").then(|| (path.to_string(), sha.to_string()))
        })
        .collect()
}

/// 按哈希读一个文件的内容（文本）。读不到返回 None。
pub fn show_blob(d: &Path, sha: &str) -> Option<String> {
    let out = git_t(&["cat-file", "-p", sha], Some(d), 120, true).ok()?;
    Some(out.trim_start_matches('\u{feff}').to_string())
}

/// 读插件清单：工作目录里的（rev 为空），或者仓库里某个提交的。sub 是插件在仓库里的子目录（空就是根目录）
pub fn read_manifests_at(d: &Path, rev: Option<&str>, sub: &str) -> Manifest {
    match rev {
        None => manifests_with(&|rel| read_json(&d.join(rel)), &|rel| d.join(rel).is_file(), sub),
        Some(r) => {
            let tree = list_tree(d, r);
            manifests_with(&|rel| tree.get(rel).and_then(|sha| show_blob(d, sha)).and_then(|t| serde_json::from_str(&t).ok()), &|rel| tree.contains_key(rel), sub)
        }
    }
}

/// 读插件清单。load 读一个 JSON 文件，has 看一个文件在不在，路径都相对仓库根目录
pub fn manifests_with(load: &dyn Fn(&str) -> Option<Value>, has: &dyn Fn(&str) -> bool, sub: &str) -> Manifest {
    let sub = sub.trim_matches('/');
    let at = |rel: &str| if sub.is_empty() || sub == "." { rel.to_string() } else { format!("{sub}/{rel}") };
    let load = |rel: &str| -> Option<Value> {
        let rel = at(rel);
        if has(&rel) { load(&rel) } else { None }
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
            if !name.is_empty() && (src == "." || src.starts_with("./")) && !src.contains("..") {
                let path = src.trim_start_matches('.').trim_matches('/');
                // 插件中心靠 ~/.claude/skills 就地加载，文件夹里得有自己的 plugin.json；只在插件源里声明、文件夹里没有清单的装不了
                let pj = if path.is_empty() { ".claude-plugin/plugin.json".to_string() } else { format!("{path}/.claude-plugin/plugin.json") };
                if has(&at(&pj)) {
                    info.claude_plugins.push(ClaudePlugin { name: name.to_string(), path: if path.is_empty() { ".".into() } else { path.to_string() } });
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn files(list: &[(&str, Value)]) -> BTreeMap<String, Value> {
        list.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn read(f: &BTreeMap<String, Value>, sub: &str) -> Manifest {
        manifests_with(&|rel| f.get(rel).cloned(), &|rel| f.contains_key(rel), sub)
    }

    #[test]
    fn marketplace_plugins_need_their_own_manifest() {
        // 插件源列了三个：一个在子目录里有 plugin.json，一个只在插件源里声明（装不了），一个指到仓库外
        let f = files(&[
            (".claude-plugin/marketplace.json", json!({"name": "m", "plugins": [
                {"name": "a", "source": "./plugins/a"}, {"name": "b", "source": "./", "skills": ["./skills/x"]}, {"name": "c", "source": {"source": "github", "repo": "o/c"}}]})),
            ("plugins/a/.claude-plugin/plugin.json", json!({"name": "a", "version": "1.2.0", "description": "A"})),
            ("plugins/a/.codex-plugin/plugin.json", json!({"name": "a"})),
        ]);
        let root = read(&f, "");
        assert_eq!(root.claude_plugins.iter().map(|c| (c.name.as_str(), c.path.as_str())).collect::<Vec<_>>(), vec![("a", "plugins/a")]);
        assert!(root.codex_name.is_none() && root.is_plugin());
        // 子目录本身就是一个插件
        let a = read(&f, "plugins/a");
        assert_eq!((a.claude_plugins.len(), a.claude_plugins[0].path.as_str(), a.codex_name.as_deref(), a.version.as_str()), (1, ".", Some("a"), "1.2.0"));
        // 只有技能、没有任何清单的仓库不算插件
        assert!(!read(&files(&[("skills/x/SKILL.md", json!(null))]), "").is_plugin());
        // 根目录就是插件：插件源写 "./" 或 "."
        let g = files(&[(".claude-plugin/marketplace.json", json!({"plugins": [{"name": "p", "source": "."}]})), (".claude-plugin/plugin.json", json!({"name": "p"}))]);
        assert_eq!(read(&g, "").claude_plugins[0].path, ".");
    }

    #[test]
    fn only_interrupted_transfers_are_retried() {
        assert!(transient("error: RPC failed; curl 18 Transferred a partial file"));
        assert!(transient("fatal: early EOF"));
        assert!(!transient("fatal: repository 'https://github.com/o/nope.git/' not found"));
        assert!(!transient("fatal: Authentication failed"));
        assert!(!transient("error: RPC failed; HTTP 403 curl 22 The requested URL returned error: 403"));
    }
}
