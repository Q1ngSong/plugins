//! 配置（config.json）和运行状态（state.json）
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::util::*;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PluginCfg {
    pub id: String,
    pub repo: String,
    pub branch: String,
    /// 装到哪些 app；没写的 app 默认装
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<BTreeMap<String, bool>>,
    /// 锁定：不检查、不拉取更新
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// 按需：不装进 app，只列在技能库里，用到时模型再去读
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub on_demand: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PluginCfg {
    pub fn target(&self, app: &str) -> bool {
        self.targets.as_ref().and_then(|t| t.get(app)).copied().unwrap_or(true)
    }

    pub fn set_target(&mut self, app: &str, on: bool) {
        self.targets.get_or_insert_with(BTreeMap::new).insert(app.to_string(), on);
    }
}

fn tolerant_int<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(60))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AutoCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "sixty", deserialize_with = "tolerant_int")]
    pub interval_minutes: i64,
    /// 旧版把后台任务的方式记在这里，新版记在 schedule
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

fn sixty() -> i64 {
    60
}

impl Default for AutoCfg {
    fn default() -> Self {
        Self { enabled: false, interval_minutes: 60, mode: None }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct GuardCfg {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ScheduleCfg {
    #[serde(default)]
    pub mode: Option<String>,
}

/// 插件中心管理的独立 skill：在 ~/.yuwanplugins/skills/<dir> 存一份，targets 里为真的 app 链接到它
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SkillCfg {
    /// SKILL.md 里写的 name
    pub name: String,
    /// 文件夹名，也是两边 skills 文件夹里链接的名字
    pub dir: String,
    #[serde(default)]
    pub targets: BTreeMap<String, bool>,
    /// 按需：两边的链接拆掉，只列在技能库里；targets 留着，切回常驻时照原样链接
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub on_demand: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default)]
    pub plugins: Vec<PluginCfg>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillCfg>,
    #[serde(default)]
    pub auto_update: AutoCfg,
    #[serde(default)]
    pub guard: GuardCfg,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<ScheduleCfg>,
    /// 桌面 App 里的代理变量，定时任务里没有，记下来给 git 用
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<BTreeMap<String, String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub fn load_config() -> Config {
    match read_json(&CONFIG_PATH) {
        Some(v @ Value::Object(_)) => match serde_json::from_value::<Config>(v) {
            Ok(c) => c,
            Err(e) => {
                log(&format!("config.json 读不了（{e}），按空配置处理"));
                Config::default()
            }
        },
        _ => Config::default(),
    }
}

pub fn save_config(cfg: &Config) -> R<()> {
    write_json(&CONFIG_PATH, &serde_json::to_value(cfg)?)
}

pub fn find_managed<'a>(cfg: &'a mut Config, pid: &str) -> R<&'a mut PluginCfg> {
    cfg.plugins.iter_mut().find(|p| p.id == pid).ok_or_else(|| HubError::Msg(format!("没有这个受管插件：{pid}")))
}

/// state.json：受管插件的检查记录、真实检查结果、后台检查的修复记录等，字段松散，直接用 JSON 对象
pub type State = Map<String, Value>;

pub fn load_state() -> State {
    let mut st = read_obj(&STATE_PATH);
    sub(&mut st, "plugins");
    st
}

pub fn save_state(st: &State) -> R<()> {
    write_json(&STATE_PATH, &Value::Object(st.clone()))
}

/// 一个受管插件在 state.json 里的记录（没有就建一个空的）
pub fn memo<'a>(st: &'a mut State, pid: &str) -> &'a mut Map<String, Value> {
    sub(sub(st, "plugins"), pid)
}

pub fn set(m: &mut Map<String, Value>, key: &str, v: impl Into<Value>) {
    m.insert(key.to_string(), v.into());
}
