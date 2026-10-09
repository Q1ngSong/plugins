//! 网上找技能：问 skills.sh 的搜索接口。只做查询，装还是走添加页那套（粘 skills.sh 的页面链接进去）。
use std::time::Duration;

use serde_json::{json, Value};

use crate::bail;
use crate::gitx::git_soft;
use crate::util::*;

const SEARCH: &str = "https://skills.sh/api/search";

/// 代理：先看环境变量（ureq 自己会读），没有就用 git 配的 http.proxy，和拉仓库走同一条路
fn agent() -> ureq::Agent {
    let mut cfg = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15)));
    let env_proxy = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"].iter().any(|k| std::env::var(k).map(|v| !v.is_empty()).unwrap_or(false));
    if !env_proxy {
        let git_proxy = git_soft(&["config", "--get", "https.proxy"], None);
        let git_proxy = if git_proxy.is_empty() { git_soft(&["config", "--get", "http.proxy"], None) } else { git_proxy };
        if let Ok(p) = ureq::Proxy::new(git_proxy.trim()) {
            cfg = cfg.proxy(Some(p));
        }
    }
    cfg.build().new_agent()
}

/// 搜 skills.sh。每条：name、source（owner/repo）、skill（仓库里的技能 id）、installs、url（skills.sh 的页面，添加页认得）
pub fn search(q: &str) -> R<Vec<Value>> {
    let q = q.trim();
    if q.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let url = format!("{SEARCH}?q={}&limit=12", urlencode(q));
    let text = (|| -> Result<String, ureq::Error> { agent().get(&url).call()?.body_mut().read_to_string() })()
        .map_err(|e| HubError::Msg(format!("连不上 skills.sh：{e}")))?;
    parse_results(&text)
}

/// skills.sh 返回的 JSON 里的 skills 数组，整理成页面要的字段
fn parse_results(text: &str) -> R<Vec<Value>> {
    let v: Value = serde_json::from_str(text).map_err(|_| HubError::Msg("skills.sh 返回的不是 JSON".into()))?;
    let Some(list) = v.get("skills").and_then(Value::as_array) else { bail!("skills.sh 返回的格式不认识") };
    Ok(list
        .iter()
        .filter(|s| !gs(s, "source").is_empty() && !gs(s, "skillId").is_empty())
        .map(|s| {
            let (source, skill) = (gs(s, "source"), gs(s, "skillId"));
            json!({
                "name": if gs(s, "name").is_empty() { skill } else { gs(s, "name") },
                "source": source, "skill": skill,
                "installs": s.get("installs").and_then(Value::as_u64).unwrap_or(0),
                "url": format!("https://skills.sh/{source}/{skill}"),
            })
        })
        .collect())
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_skills_sh_results() {
        // 2026-10 实际返回的样子，多余的字段（timings 等）忽略
        let text = r#"{"query":"pdf","searchType":"fuzzy","skills":[{"id":"anthropics/skills/pdf","source":"anthropics/skills","skillId":"pdf","name":"pdf","installs":206964},{"id":"x","source":"","skillId":"bad"}],"count":2}"#;
        let list = parse_results(text).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(gs(&list[0], "url"), "https://skills.sh/anthropics/skills/pdf");
        assert_eq!(list[0]["installs"], 206964);
        assert!(parse_results("<html>").is_err());
        assert!(parse_results("{}").is_err());
        assert_eq!(urlencode("中 a-b"), "%E4%B8%AD%20a-b");
    }
}
