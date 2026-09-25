//! 扫描两边的会话记录：每个插件被哪些项目调用过
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::bytes::Regex;
use serde::Serialize;
use serde_json::{json, Value};

use crate::util::*;

static CWD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""cwd"\s*:\s*"((?:[^"\\]|\\.)*)""#).unwrap());
/// 会话记录里的路径分隔符：/、\ 或转义过的 \\
const SEP: &str = r"(?:\\\\|/)+";
/// Claude Code 调用插件的 skill、子代理和斜杠命令时，名字都是「插件名:xxx」
static CLAUDE_USE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""(?:skill|subagent_type)"\s*:\s*"([\w.-]+):|<command-name>/?([\w.-]+):"#).unwrap());
/// Codex 用插件时会去读它缓存里的文件：.codex/plugins/cache/<插件源>/<插件>/...
static CODEX_USE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"plugins{SEP}cache{SEP}([\w.-]+){SEP}([\w.-]+){SEP}")).unwrap());
/// 受管插件的缓存链接回 ~/.yuwanplugins，Codex 读的路径就成了 .yuwanplugins/<插件>/...
static YUWAN_USE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"\.yuwanplugins{SEP}([\w.-]+){SEP}")).unwrap());

#[derive(Serialize, Clone, Debug)]
pub struct ProjRow {
    pub path: String,
    pub count: u64,
    pub last: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// 项目键（规范化路径）→ 使用情况
pub type ProjMap = BTreeMap<String, ProjRow>;

#[derive(Default)]
pub struct Usage {
    pub claude: BTreeMap<String, ProjMap>,
    pub codex: BTreeMap<String, ProjMap>,
}

impl Usage {
    pub fn get(&self, app: &str, plugin: &str) -> Option<&ProjMap> {
        if app == "claude" { self.claude.get(plugin) } else { self.codex.get(plugin) }
    }
}

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

pub fn session_cwd(path: &Path) -> Option<String> {
    let mut f = fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 262144];
    let n = std::io::Read::read(&mut f, &mut buf).ok()?;
    let m = CWD_RE.captures(&buf[..n])?;
    let quoted = format!("\"{}\"", String::from_utf8_lossy(&m[1]));
    let value: String = serde_json::from_str(&quoted).ok()?;
    if let Some(rest) = value.strip_prefix("file:///") {
        return Some(percent_decode(rest));
    }
    Some(value)
}

/// 如果是 git 工作树（.git 是文件），返回它的主仓库目录
pub fn worktree_parent(path: &Path) -> Option<String> {
    let dotgit = path.join(".git");
    if !dotgit.is_file() {
        return None;
    }
    let text = fs::read_to_string(&dotgit).ok()?;
    let gitdir = text.trim().strip_prefix("gitdir:")?.trim();
    let parts: Vec<String> = Path::new(gitdir).components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    let idx = parts.iter().rposition(|p| p == "worktrees")?;
    let mut p = PathBuf::new();
    for part in &parts[..idx] {
        p.push(part);
    }
    Some(display(p.parent()?))
}

/// 一个会话文件里调用了哪些插件、各几次。
///
/// Claude Code 按插件名记；Codex 按「插件@插件源」记，读的是 ~/.yuwanplugins 里的就记成「yuwan:<文件夹>」。
pub fn scan_session(path: &Path, app: &str) -> std::io::Result<BTreeMap<String, u64>> {
    let data = fs::read(path)?;
    let mut hits: BTreeMap<String, u64> = BTreeMap::new();
    if app == "claude" {
        for m in CLAUDE_USE.captures_iter(&data) {
            let name = m.get(1).or_else(|| m.get(2)).map(|g| String::from_utf8_lossy(g.as_bytes()).to_string()).unwrap_or_default();
            *hits.entry(name).or_insert(0) += 1;
        }
        return Ok(hits);
    }
    for (pattern, yuwan) in [(&*CODEX_USE, false), (&*YUWAN_USE, true)] {
        for m in pattern.captures_iter(&data) {
            let start = data[..m.get(0).unwrap().start()].iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0);
            let head = &data[start..(start + 250).min(data.len())];
            // 只算工具调用；提示词里的插件清单每个会话都有，不算用过
            if contains(head, br#""type":"custom_tool_call""#) || contains(head, br#""type":"function_call""#) {
                let key = if yuwan {
                    format!("yuwan:{}", String::from_utf8_lossy(&m[1]))
                } else {
                    format!("{}@{}", String::from_utf8_lossy(&m[2]), String::from_utf8_lossy(&m[1]))
                };
                *hits.entry(key).or_insert(0) += 1;
            }
        }
    }
    Ok(hits)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// 会话所在目录归到哪个项目：工作树算主仓库；临时目录、家目录不算项目
pub fn project_root(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?;
    if cwd.is_empty() {
        return None;
    }
    let p = std::path::absolute(cwd).ok()?;
    let ps = display(&p);
    if ps.contains("scratch-workspaces") || same_path(Some(&p), Some(&HOME)) || ps.len() <= 3 || !p.is_dir() {
        return None;
    }
    Some(worktree_parent(&p).unwrap_or(ps))
}

fn walk_files(root: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    if !root.is_dir() {
        return Vec::new();
    }
    walkdir::WalkDir::new(root)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file() && keep(&e.file_name().to_string_lossy()))
        .map(|e| e.into_path())
        .collect()
}

/// 从两边的会话记录统计：每个插件被哪些项目调用过。结果按文件缓存在 usage.json，文件没变就不重读。
pub fn scan_usage() -> Usage {
    let cached = read_obj(&USAGE_PATH).get("files").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut sources: Vec<(&str, PathBuf)> = walk_files(&CLAUDE_HOME.join("projects"), |n| n.ends_with(".jsonl")).into_iter().map(|p| ("claude", p)).collect();
    for folder in [CODEX_HOME.join("sessions"), CODEX_HOME.join("archived_sessions")] {
        sources.extend(walk_files(&folder, |n| n.starts_with("rollout-") && n.ends_with(".jsonl")).into_iter().map(|p| ("codex", p)));
    }
    let mut files = serde_json::Map::new();
    for (app, f) in sources {
        let Ok(info) = fs::metadata(&f) else { continue };
        let sig_m = info.modified().map(mtime_of).unwrap_or(0.0);
        let sig_s = info.len();
        let key = display(&f);
        let old = cached.get(&key);
        let same = old
            .map(|rec| {
                let sig = ga(rec, "sig");
                sig.len() == 2 && sig[0].as_f64() == Some(sig_m) && sig[1].as_u64() == Some(sig_s)
            })
            .unwrap_or(false);
        let rec = if same {
            old.cloned().unwrap()
        } else {
            let Ok(hits) = scan_session(&f, app) else { continue };
            let cwd = if hits.is_empty() { None } else { session_cwd(&f) };
            json!({"sig": [sig_m, sig_s], "app": app, "hits": hits, "cwd": cwd})
        };
        files.insert(key, rec);
    }
    if files != cached {
        let _ = write_json(&USAGE_PATH, &json!({"files": files}));
    }
    let mut usage = Usage::default();
    for rec in files.values() {
        let hits = go(rec, "hits").filter(|h| !h.is_empty());
        let Some(hits) = hits else { continue };
        let Some(root) = project_root(rec.get("cwd").and_then(Value::as_str)) else { continue };
        let last = ga(rec, "sig").first().and_then(Value::as_f64).unwrap_or(0.0);
        let table = if gs(rec, "app") == "claude" { &mut usage.claude } else { &mut usage.codex };
        for (plugin, n) in hits {
            let row = table
                .entry(plugin.clone())
                .or_default()
                .entry(norm(Path::new(&root)))
                .or_insert_with(|| ProjRow { path: root.clone(), count: 0, last: 0.0, scope: None });
            row.count += n.as_u64().unwrap_or(0);
            if last > row.last {
                row.last = last;
            }
        }
    }
    usage
}

/// 合并几组「项目 → 使用情况」，同一项目的次数相加、最近时间取最新
pub fn merge_projects(groups: &[Option<&ProjMap>]) -> ProjMap {
    let mut out = ProjMap::new();
    for g in groups.iter().flatten() {
        for (k, v) in g.iter() {
            let row = out.entry(k.clone()).or_insert_with(|| ProjRow { path: v.path.clone(), count: 0, last: 0.0, scope: None });
            row.count += v.count;
            if v.last > row.last {
                row.last = v.last;
            }
        }
    }
    out
}

pub fn project_rows(rows: &ProjMap) -> Vec<Value> {
    let mut list: Vec<&ProjRow> = rows.values().collect();
    list.sort_by(|a, b| b.last.partial_cmp(&a.last).unwrap_or(std::cmp::Ordering::Equal));
    list.into_iter().map(|r| serde_json::to_value(r).unwrap_or(Value::Null)).collect()
}
