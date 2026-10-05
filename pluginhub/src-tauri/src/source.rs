//! 「添加」框里粘进来的东西怎么认：GitHub 的各种写法、仓库里子目录或技能的链接、npx skills add 命令、
//! 两边命令行的 marketplace add 命令、skills.sh 的页面链接。这里只认格式、不联网。
//! /tree/ 后面哪段是分支、哪段是子目录，要拿到分支列表才能定，留给探测时的 resolve_ref。
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

use crate::bail;
use crate::gitx::repo_web_url;
use crate::util::*;

const UNKNOWN: &str = "看不懂这个地址或命令。支持：GitHub 链接或 owner/repo（可带 #分支）、仓库里子目录或技能的链接、npx skills add … 命令、skills.sh 的页面链接，以及 claude / codex 的 plugin marketplace add 命令。";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Via {
    #[default]
    Url,
    Shorthand,
    LocalPath,
    Npx,
    SkillsSh,
    ClaudeCli,
    CodexCli,
    SkillInstaller,
    GitClone,
}

impl Via {
    pub fn name(self) -> &'static str {
        match self {
            Via::Url => "url",
            Via::Shorthand => "shorthand",
            Via::LocalPath => "local",
            Via::Npx => "npx",
            Via::SkillsSh => "skills-sh",
            Via::ClaudeCli => "claude-cli",
            Via::CodexCli => "codex-cli",
            Via::SkillInstaller => "skill-installer",
            Via::GitClone => "git-clone",
        }
    }
}

/// 认出来的来源
#[derive(Debug, Clone, Default)]
pub struct Source {
    /// 克隆地址
    pub repo: String,
    /// 网页地址，显示用
    pub web: String,
    /// GitHub 上的 (owner, repo)
    pub github: Option<(String, String)>,
    /// GitHub 链接 /tree/ 或 /blob/ 后面的部分：分支和子目录连在一起，还没拆开
    pub tree: Option<String>,
    /// 链接指向的是文件（/blob/），子目录取它所在的文件夹
    pub file: bool,
    /// 明确给的分支或标签：#ref、@ref、--ref、--branch
    pub ref_hint: Option<String>,
    /// 点名的技能（名字或文件夹名），"*" 表示全部
    pub skills: Vec<String>,
    /// 点名的插件（/plugin install 名字 --marketplace …）
    pub plugin: Option<String>,
    /// 命令里指定的 app，已换成 claude / codex
    pub agents: Vec<String>,
    pub via: Via,
    /// 识别时的提醒，比如命令里有插件中心不管的 agent
    pub notes: Vec<String>,
}

impl Source {
    /// 用户要的是技能，不是整个插件
    pub fn wants_skills(&self) -> bool {
        matches!(self.via, Via::Npx | Via::SkillInstaller | Via::SkillsSh) || !self.skills.is_empty()
    }

    /// 给页面看的一句话
    pub fn describe(&self) -> String {
        let what = match (&self.github, self.via) {
            (Some((o, r)), _) => format!("GitHub 仓库 {o}/{r}"),
            (None, Via::LocalPath) => format!("本地仓库 {}", self.repo),
            (None, _) => format!("Git 仓库 {}", self.repo),
        };
        let mut parts = vec![what];
        if let Some(t) = &self.tree {
            parts.push(format!("链接指向{}{t}", if self.file { "文件 " } else { " " }));
        }
        if let Some(r) = &self.ref_hint {
            parts.push(format!("分支 {r}"));
        }
        if self.skills.iter().any(|s| s == "*") {
            parts.push("全部技能".into());
        } else if !self.skills.is_empty() {
            parts.push(format!("技能 {}", self.skills.join("、")));
        }
        if let Some(p) = &self.plugin {
            parts.push(format!("插件 {p}"));
        }
        let prefix = match self.via {
            Via::Npx => "npx skills add 命令：",
            Via::SkillsSh => "skills.sh 的页面：",
            Via::ClaudeCli => "Claude Code 的命令：",
            Via::CodexCli => "Codex 的命令：",
            Via::SkillInstaller => "$skill-installer 命令：",
            Via::GitClone => "git clone 命令：",
            _ => "",
        };
        format!("{prefix}{}", parts.join("，"))
    }
}

static GITHUB_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?i)(?:https?://)?(?:www\.)?github\.com/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+?)(?:\.git)?(?:/(.*))?$").unwrap());
static GITHUB_SSH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?i)(?:ssh://)?git@github\.com[:/]([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+?)(?:\.git)?$").unwrap());
static SKILLS_SH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?i)(?:https?://)?(?:www\.)?skills\.sh/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)(?:/([A-Za-z0-9_.-]+))?/?$").unwrap());
static SHORTHAND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+?)(?:\.git)?(?:@([^@\s]+))?$").unwrap());
static GENERIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?i)(https?://|git@|ssh://|git://|file://)").unwrap());

fn github(owner: &str, repo: &str, via: Via) -> Source {
    Source {
        repo: format!("https://github.com/{owner}/{repo}.git"),
        web: format!("https://github.com/{owner}/{repo}"),
        github: Some((owner.to_string(), repo.to_string())),
        via,
        ..Default::default()
    }
}

/// 链接里的 %20 这种还原回来
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 一个地址：GitHub 的各种写法、别的 git 地址、本地路径
fn locator(raw: &str) -> R<Source> {
    let s = raw.trim();
    if s.is_empty() {
        bail!("{UNKNOWN}");
    }
    // 本地路径
    let windows_drive = s.len() > 2 && s.as_bytes()[1] == b':' && (s.as_bytes()[2] == b'\\' || s.as_bytes()[2] == b'/');
    if s.starts_with("./") || s.starts_with("../") || s.starts_with("~/") || s.starts_with('/') || s == "." || windows_drive {
        let p = match s.strip_prefix("~/") {
            Some(rest) => HOME.join(rest),
            None => PathBuf::from(s),
        };
        let abs = std::path::absolute(&p).unwrap_or(p);
        let text = display(&abs).replace('\\', "/");
        let url = if text.starts_with('/') { format!("file://{text}") } else { format!("file:///{text}") };
        return Ok(Source { repo: url.clone(), web: url, via: Via::LocalPath, ..Default::default() });
    }
    // #ref 单独拿出来；网页链接的 ?query 不要
    let (body, frag) = match s.split_once('#') {
        Some((b, f)) if !f.is_empty() => (b, Some(f.to_string())),
        Some((b, _)) => (b, None),
        None => (s, None),
    };
    let body = body.split('?').next().unwrap_or(body).trim_end_matches('/');
    if let Some(m) = GITHUB_URL.captures(body) {
        let mut src = github(&m[1], &m[2], Via::Url);
        let rest = m.get(3).map(|x| x.as_str().trim_matches('/')).unwrap_or("");
        if !rest.is_empty() {
            let (kind, tail) = rest.split_once('/').unwrap_or((rest, ""));
            let tail = percent_decode(tail.trim_matches('/'));
            match kind {
                "tree" | "src" => {
                    if !tail.is_empty() {
                        src.tree = Some(tail);
                    }
                }
                "blob" | "raw" => {
                    src.tree = Some(tail);
                    src.file = true;
                }
                "commit" | "commits" => src.ref_hint = tail.split('/').next().filter(|x| !x.is_empty()).map(str::to_string),
                "releases" => src.ref_hint = tail.strip_prefix("tag/").map(|t| t.split('/').next().unwrap_or(t).to_string()),
                _ => src.notes.push(format!("链接里的 /{kind}/… 这部分没用上，按整个仓库处理")),
            }
        }
        if src.ref_hint.is_none() {
            src.ref_hint = frag;
        }
        return Ok(src);
    }
    if let Some(m) = GITHUB_SSH.captures(body) {
        let mut src = github(&m[1], &m[2], Via::Url);
        src.repo = format!("git@github.com:{}/{}.git", &m[1], &m[2]); // 用 SSH 的保留 SSH，私有仓库靠它认证
        src.ref_hint = frag;
        return Ok(src);
    }
    if let Some(m) = SKILLS_SH.captures(body) {
        let mut src = github(&m[1], &m[2], Via::SkillsSh);
        if let Some(skill) = m.get(3) {
            src.skills.push(skill.as_str().to_string());
        }
        return Ok(src);
    }
    if let Some(m) = SHORTHAND.captures(body) {
        if &m[1] != "." && &m[1] != ".." {
            let mut src = github(&m[1], &m[2], Via::Shorthand);
            src.ref_hint = m.get(3).map(|x| x.as_str().to_string()).or(frag);
            return Ok(src);
        }
    }
    if GENERIC.is_match(body) {
        return Ok(Source { repo: body.to_string(), web: repo_web_url(body), ref_hint: frag, via: Via::Url, ..Default::default() });
    }
    bail!("{UNKNOWN}")
}

/// 按 shell 的规矩拆开一行：引号里的空格不算分隔
fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut quoted = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                quoted = true;
            }
            None if c.is_whitespace() => {
                if quoted || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    quoted = false;
                }
            }
            None => cur.push(c),
        }
    }
    if quoted || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 带值的参数：--skill x、--skill=x、-s x 都认
fn flag_value(args: &[String], i: &mut usize, inline: Option<&str>) -> Option<String> {
    if let Some(v) = inline {
        return Some(v.to_string());
    }
    let v = args.get(*i + 1).filter(|v| !v.starts_with('-')).cloned();
    if v.is_some() {
        *i += 1;
    }
    v
}

/// npx skills add <来源> [--skill a,b] [-a claude-code,codex] [-g] …（也认 bunx / pnpm dlx / npx add-skill）
fn parse_npx(args: &[String]) -> R<Source> {
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        i += if args[i] == "-p" || args[i] == "--package" { 2 } else { 1 };
    }
    let Some(pkg) = args.get(i) else { bail!("npx 后面要跟 skills add …") };
    let name = match pkg.strip_prefix('@') {
        Some(scoped) => format!("@{}", scoped.split('@').next().unwrap_or(scoped)),
        None => pkg.split('@').next().unwrap_or(pkg).to_string(),
    };
    if !["skills", "add-skill", "skills-cli", "@vercel-labs/skills"].contains(&name.as_str()) {
        bail!("只认识 npx skills add … 这种装技能的命令，{pkg} 不认识。");
    }
    i += 1;
    if name != "add-skill" {
        match args.get(i).map(String::as_str) {
            Some("add" | "install" | "i") => i += 1,
            Some(other) => bail!("npx skills {other} 不是安装命令，插件中心只处理 npx skills add …"),
            None => bail!("npx skills add 后面要跟仓库地址。"),
        }
    }
    let (mut target, mut skills, mut agents) = (None, Vec::new(), Vec::new());
    while i < args.len() {
        let a = args[i].clone();
        let (key, inline) = match a.split_once('=') {
            Some((k, v)) if k.starts_with('-') => (k.to_string(), Some(v)),
            _ => (a.clone(), None),
        };
        match key.as_str() {
            "--skill" | "--skills" | "-s" => {
                if let Some(v) = flag_value(args, &mut i, inline) {
                    skills.extend(v.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string));
                }
            }
            "--agent" | "--agents" | "-a" => {
                if let Some(v) = flag_value(args, &mut i, inline) {
                    agents.extend(v.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string));
                }
            }
            "--all" => skills.push("*".into()),
            k if k.starts_with('-') => {} // -g -y --copy --list … 插件中心用不上
            _ => {
                if target.is_none() {
                    target = Some(a);
                }
            }
        }
        i += 1;
    }
    let Some(target) = target else { bail!("npx skills add 后面要跟仓库地址。") };
    let mut src = locator(&target)?;
    src.via = Via::Npx;
    for s in skills {
        if !src.skills.contains(&s) {
            src.skills.push(s);
        }
    }
    for a in agents {
        let app = match a.to_lowercase().as_str() {
            "claude-code" | "claude" | "claude_code" | "claudecode" => "claude",
            "codex" => "codex",
            other => {
                src.notes.push(format!("命令里的 agent「{other}」插件中心不管，只装到 Claude Code 和 Codex"));
                continue;
            }
        };
        if !src.agents.iter().any(|x| x == app) {
            src.agents.push(app.to_string());
        }
    }
    Ok(src)
}

enum Line {
    Source(Source),
    /// 只有「插件名@插件源名」，没有仓库地址
    PluginOnly(String),
}

/// claude plugin … / /plugin … 后面的部分
fn parse_claude(args: &[String]) -> R<Line> {
    match args.first().map(String::as_str) {
        Some("marketplace" | "market") if args.get(1).map(String::as_str) == Some("add") => {
            let Some(target) = args.get(2) else { bail!("marketplace add 后面要跟地址。") };
            let mut src = locator(target)?;
            src.via = Via::ClaudeCli;
            Ok(Line::Source(src))
        }
        Some("install" | "i") => {
            let Some(sel) = args.get(1) else { bail!("{UNKNOWN}") };
            let name = sel.split('@').next().unwrap_or(sel).to_string();
            if let Some(pos) = args.iter().position(|a| a == "--marketplace" || a == "-m") {
                if let Some(target) = args.get(pos + 1) {
                    let mut src = locator(target)?;
                    src.via = Via::ClaudeCli;
                    src.plugin = Some(name);
                    return Ok(Line::Source(src));
                }
            }
            Ok(Line::PluginOnly(sel.clone()))
        }
        _ => bail!("{UNKNOWN}"),
    }
}

/// codex plugin … 后面的部分
fn parse_codex(args: &[String]) -> R<Line> {
    match args.first().map(String::as_str) {
        Some("marketplace") if args.get(1).map(String::as_str) == Some("add") => {
            let Some(target) = args.get(2) else { bail!("marketplace add 后面要跟地址。") };
            let mut src = locator(target)?;
            src.via = Via::CodexCli;
            if let Some(pos) = args.iter().position(|a| a == "--ref") {
                if let Some(r) = args.get(pos + 1) {
                    src.ref_hint = Some(r.clone());
                }
            }
            Ok(Line::Source(src))
        }
        Some("add") => match args.get(1) {
            Some(sel) => Ok(Line::PluginOnly(sel.clone())),
            None => bail!("{UNKNOWN}"),
        },
        _ => bail!("{UNKNOWN}"),
    }
}

/// $skill-installer install <GitHub 链接>：找到里面的地址就行
fn parse_skill_installer(args: &[String]) -> R<Source> {
    for a in args {
        if GITHUB_URL.is_match(a.trim_end_matches('/')) || SHORTHAND.is_match(a) {
            if let Ok(mut src) = locator(a) {
                src.via = Via::SkillInstaller;
                return Ok(src);
            }
        }
    }
    bail!("$skill-installer 后面要有 GitHub 的仓库地址或者 /tree/ 链接。")
}

fn parse_git_clone(args: &[String]) -> R<Source> {
    let mut i = 0;
    let mut branch = None;
    let mut target = None;
    while i < args.len() {
        let a = &args[i];
        if a == "--branch" || a == "-b" {
            branch = args.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if !a.starts_with('-') && target.is_none() {
            target = Some(a.clone());
        }
        i += 1;
    }
    let Some(target) = target else { bail!("git clone 后面要跟地址。") };
    let mut src = locator(&target)?;
    src.via = Via::GitClone;
    if branch.is_some() {
        src.ref_hint = branch;
    }
    Ok(src)
}

fn parse_line(line: &str) -> R<Line> {
    let mut t = tokens(line);
    if t.first().map(|x| x == "$" || x == ">").unwrap_or(false) {
        t.remove(0); // 粘进来的提示符
    }
    if t.is_empty() {
        bail!("{UNKNOWN}");
    }
    let head = t[0].to_lowercase();
    let rest = &t[1..];
    let first = |x: &str| rest.first().map(|r| r == x).unwrap_or(false);
    let src = match head.as_str() {
        "npx" | "bunx" | "pnpx" => parse_npx(rest)?,
        "pnpm" | "yarn" if first("dlx") => parse_npx(&rest[1..])?,
        "npm" if first("exec") || first("x") => parse_npx(&rest[1..].iter().filter(|x| *x != "--").cloned().collect::<Vec<_>>())?,
        "claude" if first("plugin") || first("plugins") => return parse_claude(&rest[1..]),
        "/plugin" | "/plugins" => return parse_claude(rest),
        "codex" if first("plugin") => return parse_codex(&rest[1..]),
        "$skill-installer" | "skill-installer" => parse_skill_installer(rest)?,
        "git" if first("clone") => parse_git_clone(&rest[1..])?,
        _ => {
            if t.len() > 1 {
                bail!("{UNKNOWN}");
            }
            locator(&t[0])?
        }
    };
    Ok(Line::Source(src))
}

/// 认一段粘进来的文字。可以是好几行（比如 marketplace add 加 install 两行），取第一个带地址的；
/// 只写了「插件名@插件源名」的行，记下插件名
pub fn parse(input: &str) -> R<Source> {
    let text = input.replace("\\\r\n", " ").replace("\\\n", " ");
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    if lines.is_empty() {
        bail!("贴一个 GitHub 仓库地址（或 owner/repo），或者 npx skills add … 命令。");
    }
    let mut found: Option<Source> = None;
    let mut plugin_hint: Option<String> = None;
    let mut first_err: Option<HubError> = None;
    for line in &lines {
        match parse_line(line) {
            Ok(Line::Source(s)) => {
                if found.is_none() {
                    found = Some(s);
                }
            }
            Ok(Line::PluginOnly(sel)) => {
                if plugin_hint.is_none() {
                    plugin_hint = Some(sel);
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    let Some(mut src) = found else {
        if let Some(sel) = plugin_hint {
            bail!("「{sel}」只有插件名和插件源名，没有仓库地址。把插件源的地址（marketplace add 那一行，或者 GitHub 链接）也贴进来。");
        }
        return Err(first_err.unwrap_or_else(|| HubError::Msg(UNKNOWN.into())));
    };
    if src.plugin.is_none() {
        src.plugin = plugin_hint.map(|sel| sel.split('@').next().unwrap_or(&sel).to_string());
    }
    Ok(src)
}

/// 两个写法是不是同一个仓库：大小写、.git 后缀、SSH 和 HTTPS 的差别都不算
pub fn same_repo(a: &str, b: &str) -> bool {
    fn key(s: &str) -> String {
        let mut s = s.trim().trim_end_matches('/').to_lowercase();
        if let Some(rest) = s.strip_prefix("ssh://git@") {
            s = format!("https://{rest}");
        } else if let Some(rest) = s.strip_prefix("git@") {
            s = format!("https://{}", rest.replacen(':', "/", 1));
        }
        let s = s.strip_suffix(".git").unwrap_or(&s).to_string();
        let s = s.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
        s.to_string()
    }
    key(a) == key(b)
}

/// /tree/ 后面的那串拆成分支和子目录：按分支列表，取能对上的最长的那个分支名。返回 (分支, 子目录, 对上了没有)。
pub fn split_tree(tree: &str, branches: &[String]) -> (String, Option<String>, bool) {
    let t = tree.trim_matches('/');
    let mut best: Option<&String> = None;
    for b in branches {
        if (t == b || t.starts_with(&format!("{b}/"))) && best.map(|x| b.len() > x.len()).unwrap_or(true) {
            best = Some(b);
        }
    }
    match best {
        Some(b) => {
            let rest = t[b.len()..].trim_matches('/');
            (b.clone(), (!rest.is_empty()).then(|| rest.to_string()), true)
        }
        None => match t.split_once('/') {
            Some((r, rest)) => (r.to_string(), (!rest.is_empty()).then(|| rest.trim_matches('/').to_string()), false),
            None => (t.to_string(), None, false),
        },
    }
}

/// 拿到分支列表以后，定下来用哪个分支、子目录是什么。返回 (分支, 子目录, 提醒)。
pub fn resolve_ref(src: &Source, branches: &[String], default: &str) -> (String, Option<String>, Option<String>) {
    let not_branch = |r: &str| Some(format!("{r} 不是分支（可能是标签或提交号），先按 {default} 分支看；要跟别的分支就在下面选"));
    if let Some(tree) = src.tree.as_deref().map(|t| t.trim_matches('/')).filter(|t| !t.is_empty()) {
        let (r, mut path, matched) = split_tree(tree, branches);
        if src.file {
            // 文件所在的文件夹；文件在根目录就是根目录
            path = path.and_then(|p| p.rsplit_once('/').map(|(d, _)| d.to_string()));
        }
        return if matched { (r, path, None) } else { (default.to_string(), path, not_branch(&r)) };
    }
    if let Some(h) = &src.ref_hint {
        return if branches.iter().any(|b| b == h) { (h.clone(), None, None) } else { (default.to_string(), None, not_branch(h)) };
    }
    (default.to_string(), None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_forms() {
        let s = parse("anthropics/skills").unwrap();
        assert_eq!(s.repo, "https://github.com/anthropics/skills.git");
        assert_eq!(s.github, Some(("anthropics".into(), "skills".into())));
        assert_eq!(s.via, Via::Shorthand);
        assert_eq!(parse("owner/repo#dev").unwrap().ref_hint.as_deref(), Some("dev"));
        assert_eq!(parse("owner/repo@v1.2").unwrap().ref_hint.as_deref(), Some("v1.2"));
        let u = parse("https://github.com/Q1ngSong/design-code-like-a-human/").unwrap();
        assert_eq!(u.repo, "https://github.com/Q1ngSong/design-code-like-a-human.git");
        assert_eq!(u.web, "https://github.com/Q1ngSong/design-code-like-a-human");
        let t = parse("https://github.com/anthropics/skills/tree/main/skills/pdf?tab=readme").unwrap();
        assert_eq!(t.tree.as_deref(), Some("main/skills/pdf"));
        assert!(!t.file);
        let b = parse("https://github.com/o/r/blob/main/skills/pdf/SKILL.md").unwrap();
        assert!(b.file);
        assert_eq!(b.tree.as_deref(), Some("main/skills/pdf/SKILL.md"));
        let ssh = parse("git@github.com:o/r.git").unwrap();
        assert_eq!(ssh.repo, "git@github.com:o/r.git");
        assert_eq!(ssh.web, "https://github.com/o/r");
        assert_eq!(parse("./x/y").unwrap().via, Via::LocalPath);
        let g = parse("https://gitlab.example.com/g/m.git#v1").unwrap();
        assert_eq!((g.repo.as_str(), g.ref_hint.as_deref()), ("https://gitlab.example.com/g/m.git", Some("v1")));
        let sk = parse("https://skills.sh/vercel-labs/agent-skills/vercel-react-best-practices").unwrap();
        assert_eq!((sk.via, sk.skills.clone()), (Via::SkillsSh, vec!["vercel-react-best-practices".to_string()]));
        assert!(parse("random words here").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn tree_split_and_resolve() {
        let names = vec!["main".to_string(), "feature/x".to_string()];
        assert_eq!(split_tree("main/skills/pdf", &names), ("main".into(), Some("skills/pdf".into()), true));
        assert_eq!(split_tree("feature/x/skills", &names), ("feature/x".into(), Some("skills".into()), true));
        assert_eq!(split_tree("v1.0/skills", &names), ("v1.0".into(), Some("skills".into()), false));
        assert_eq!(split_tree("main", &names), ("main".into(), None, true));
        let b = parse("https://github.com/o/r/blob/main/SKILL.md").unwrap();
        assert_eq!(resolve_ref(&b, &names, "main"), ("main".into(), None, None));
        let t = parse("https://github.com/o/r/tree/v2/skills/a").unwrap();
        let (r, p, note) = resolve_ref(&t, &names, "main");
        assert_eq!((r.as_str(), p.as_deref()), ("main", Some("skills/a")));
        assert!(note.is_some());
    }

    #[test]
    fn npx_commands() {
        let s = parse("npx skills add vercel-labs/agent-skills --skill vercel-react-best-practices -a claude-code,codex -g").unwrap();
        assert_eq!(s.via, Via::Npx);
        assert_eq!(s.github.as_ref().unwrap().1, "agent-skills");
        assert_eq!(s.skills, vec!["vercel-react-best-practices"]);
        assert_eq!(s.agents, vec!["claude", "codex"]);
        let s = parse("$ npx -y skills@latest add https://github.com/o/r/tree/main/skills/a -s a,b --skill=c").unwrap();
        assert_eq!(s.skills, vec!["a", "b", "c"]);
        assert_eq!(s.tree.as_deref(), Some("main/skills/a"));
        assert_eq!(parse("bunx skills add o/r --all").unwrap().skills, vec!["*"]);
        assert_eq!(parse("pnpm dlx skills add o/r -y").unwrap().via, Via::Npx);
        assert_eq!(parse("npx add-skill o/r").unwrap().via, Via::Npx);
        let s = parse("npx skills add o/r -a cursor").unwrap();
        assert!(s.agents.is_empty() && !s.notes.is_empty());
        assert!(parse("npx create-react-app foo").is_err());
        assert!(parse("npx skills find pdf").is_err());
        assert!(parse("npx skills add").is_err());
    }

    #[test]
    fn cli_commands() {
        assert_eq!(parse("claude plugin marketplace add anthropics/claude-plugins-official").unwrap().via, Via::ClaudeCli);
        let s = parse("/plugin marketplace add https://gitlab.example.com/g/m.git#v1.0.0").unwrap();
        assert_eq!((s.repo.as_str(), s.ref_hint.as_deref()), ("https://gitlab.example.com/g/m.git", Some("v1.0.0")));
        assert_eq!(parse("/plugin install deploy-helper --marketplace your-org/plugins").unwrap().plugin.as_deref(), Some("deploy-helper"));
        let two = parse("claude plugin marketplace add o/r\nclaude plugin install thing@r").unwrap();
        assert_eq!((two.github.as_ref().unwrap().1.as_str(), two.plugin.as_deref()), ("r", Some("thing")));
        assert!(parse("/plugin install thing@mkt").is_err());
        let c = parse("codex plugin marketplace add o/r@dev --sparse plugins").unwrap();
        assert_eq!((c.via, c.ref_hint.as_deref()), (Via::CodexCli, Some("dev")));
        assert_eq!(parse("codex plugin marketplace add https://github.com/o/r.git --ref main").unwrap().ref_hint.as_deref(), Some("main"));
        assert!(parse("codex plugin add thing@mkt").is_err());
        let si = parse("$skill-installer install https://github.com/openai/skills/tree/main/skills/.experimental/create-plan").unwrap();
        assert_eq!((si.via, si.tree.as_deref()), (Via::SkillInstaller, Some("main/skills/.experimental/create-plan")));
        assert_eq!(parse("git clone --branch dev https://github.com/o/r.git").unwrap().ref_hint.as_deref(), Some("dev"));
    }

    #[test]
    fn same_repo_spellings() {
        assert!(same_repo("https://github.com/O/R.git", "git@github.com:o/r"));
        assert!(same_repo("ssh://git@github.com/o/r.git", "https://www.github.com/o/r/"));
        assert!(!same_repo("https://github.com/a/r", "https://github.com/b/r"));
    }
}
