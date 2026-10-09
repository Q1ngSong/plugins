/* 调用插件中心的接口。在窗口里直接调用程序内部的 api 命令；在浏览器里走页面服务（令牌在 index.html 里注入） */

export type AppKey = "claude" | "codex";
export const APP_NAME: Record<AppKey, string> = { claude: "Claude Code", codex: "Codex" };
export const APPS: AppKey[] = ["claude", "codex"];

export interface Project { path: string; count: number; last: number; scope?: string }

export interface AppEntry {
  app: AppKey;
  id: string;
  installed: boolean;
  version: string | null;
  enabled: boolean | null;
  source: string;
  description: string;
  projects: Project[];
  official: boolean;
  path?: string | null;
  /** 从另一个 app 移植过来的副本才有 */
  port?: { from: AppKey; source_id: string; source_version: string; folder: string; stale?: boolean; source_missing?: boolean } | null;
  state?: "ok" | "outdated" | "unlinked" | "disabled" | "missing";
  linked?: boolean;
  commit?: string | null;
  /** 上次真实检查：让 app 自己确认看得到插件的 skill */
  verify?: { ok: boolean; detail: string; at: string } | null;
  /** 插件中心装的那一份哪里不对（配置被改掉、链接断了等），好的时候是空的 */
  problems?: string[];
  /** 后台检查最近一次发现问题并修复的记录 */
  repair?: { at: string; what: string[]; ok: boolean } | null;
  /** 独立的 skill：app 那边的文件夹实际指向哪里（linked 表示它是链接） */
  target?: string | null;
}

/**
 * 还没装的 app 能怎么装：native 直接装；adopt 先交给插件中心管理；port 移植 skill；
 * 独立的 skill 用 link（链接到统一存放的那份）和 move（先挪进统一存放的地方）；null 装不了（why 是原因）
 */
interface InstallOption { how: "native" | "adopt" | "port" | "link" | "move" | null; from?: AppKey; why?: string }

interface Commit { sha: string; short: string; subject: string; date: string }

interface Managed {
  id: string;
  /** 插件文件夹，克隆下来之后才有 */
  folder?: string;
  /** 插件在仓库里的子目录，没有就是根目录 */
  path?: string | null;
  repo: string;
  web: string;
  slug: string;
  branch: string;
  checked_at: string | null;
  error: string | null;
  cloned: boolean;
  needs_update: boolean;
  /** 锁定：不检查、不拉取更新 */
  locked: boolean;
  /** ~/.yuwanplugins 里这个插件文件夹的修改（没提交的改动、本地提交等），没有修改是空的 */
  modified: string[];
  head?: Commit | null;
  remote?: Commit | null;
}

export type Kind = "plugin" | "skill";

export interface Plugin {
  key: string;
  /** 插件，还是独立的 skill（不属于任何插件、单独放在 app 的 skills 文件夹里） */
  kind: Kind;
  /** 独立的 skill 才有：ours 表示已经统一存放在 ~/.yuwanplugins/skills */
  /** repo/path：从受管仓库里拿的技能，仓库的 id 和技能在仓库里的文件夹 */
  skill?: { ours: boolean; folder: string | null; dir: string; repo?: string | null; path?: string | null } | null;
  name: string;
  version: string;
  description: string;
  managed: Managed | null;
  repo: string;
  link: string;
  official: boolean;
  supports: AppKey[];
  install: Partial<Record<AppKey, InstallOption>>;
  installable: AppKey[];
  apps: AppEntry[];
  /** 每次会话常驻的开销：技能的名字和说明（粗估 token） */
  cost?: number;
}

export function howText(o: InstallOption): string {
  if (o.how === "native") return "直接安装";
  if (o.how === "adopt") return "克隆到 ~/.yuwanplugins 交给插件中心管理，再装进去";
  if (o.how === "port") return `从 ${APP_NAME[o.from ?? "codex"]} 移植 skill：复制一份到 ~/.yuwanplugins 再链接进去`;
  if (o.how === "link") return "链接到 ~/.yuwanplugins/skills 里统一存放的那份";
  if (o.how === "move") return `把 ${APP_NAME[o.from ?? "codex"]} 里的那份挪进 ~/.yuwanplugins/skills 统一存放，两边都链接过去`;
  return o.why ?? "装不了";
}

interface AutoUpdate { enabled: boolean; interval_minutes: number; installed: boolean; mode: string | null; next_run?: string }

export interface State {
  generated_at: string;
  hub: {
    version: string;
    /** 程序跑在哪个系统上：windows、macos、linux */
    os: string;
    home: string; plugins_dir: string; saved_dir: string; auto: AutoUpdate; last_auto: { at: string; notes: string[] } | null;
    /** 后台检查：定时看两边的插件配置有没有被改掉 */
    guard: { enabled: boolean; last: string | null };
    /** 后台任务实际运行的命令（装了桌面版就是它的 exe） */
    launcher: string;
  };
  tools: Record<AppKey | "git", string>;
  /** 上次看 Codex 的技能清单：超了上限时 ok 为假，detail 写着几条说明被截短 */
  codex_skills: { ok: boolean; detail: string; at: string } | null;
  plugins: Plugin[];
  log: string[];
}

/** 插件或技能存放在哪：插件中心管的那份；不归它管的就是装了它的 app 里那份，是链接的话取指向的文件夹 */
export function storageFolder(p: Plugin): string | null {
  // 从仓库装的技能两样都有：打开技能自己的文件夹，不是整个克隆
  if (p.skill?.folder) return p.skill.folder;
  if (p.managed?.folder) return p.managed.folder;
  const a = p.apps.find((x) => x.installed && (x.target || x.path));
  return a ? a.target || a.path || null : null;
}

/** 两边命令行的文件名：Windows 上带 .exe */
export const exeName = (state: State, app: AppKey) => (state.hub.os === "windows" ? `${app}.exe` : app);
/** 系统定时任务在这个系统上叫什么 */
export const taskKind = (state: State) => (state.hub.os === "macos" ? "launchd 任务" : "系统定时任务");
/** 系统的文件管理器叫什么（弹窗文案里用） */
export const fileManager = (state: State) => (state.hub.os === "windows" ? "资源管理器" : state.hub.os === "macos" ? "访达" : "文件管理器");

/** 查询时在仓库里找到的一个技能。local：本机哪里已经有同名的（agents 是 ~/.agents/skills，Codex 也读它） */
export interface ProbeSkill { name: string; path: string; description: string; version: string; local: (AppKey | "agents")[] }
/** 仓库的插件源里列的一个插件：path 是它在仓库里的子目录（. 是根目录），codex 表示那里也有 Codex 的清单 */
export interface ProbePlugin { name: string; path: string; description: string; version: string; codex: boolean }

export interface Branch {
  name: string; sha: string; date: string; subject: string; default: boolean;
  version: string; title: string; description: string; claude: boolean; codex: boolean;
  /** 能装进 Claude Code 的插件：插件源里列的、文件夹里有 plugin.json 的 */
  plugins: ProbePlugin[];
  /** 链接或命令指到了某个子目录、那里是插件时，这个分支上插件所在的子目录 */
  plugin_path: string | null;
  /** 指到的子目录里是什么 */
  at_path: { plugin: boolean; skill: boolean } | null;
  /** 仓库里的技能（最多列 200 个）和总数 */
  skills: ProbeSkill[];
  skills_total: number;
}

export interface Probe {
  repo: string;
  web: string;
  id: string;
  /** 这个仓库已经在管理列表里：装成了插件，还是只拿了里面的技能；跟的是哪个分支 */
  managed: boolean;
  managed_as: "plugin" | "skills" | null;
  managed_branch: string | null;
  /** 已经从这个仓库装了的技能（仓库里的路径） */
  managed_skills: string[];
  /** 建议的装法：是插件就装插件；点名了技能、链接指到技能、仓库里只有技能就装技能 */
  mode: "plugin" | "skills" | "none";
  /** 链接或命令里指定的分支，没指定就是默认分支 */
  ref: string;
  /** 链接指到的子目录 */
  path: string | null;
  /** 查询时要提醒的话 */
  notes: string[];
  /** 粘进来的东西识别成了什么 */
  input: { via: string; text: string; skills: string[]; plugin: string | null; agents: AppKey[]; notes: string[] };
  /** 细看过的分支：默认分支、点名的、已经在跟的，加上最近更新的几个；branches_total 是仓库的分支总数 */
  branches: Branch[];
  branches_total: number;
}

/** skills.sh 上搜到的一个技能：url 是它在 skills.sh 的页面，添加页认得 */
export interface WebSkill { name: string; source: string; skill: string; installs: number; url: string }

/** 改动类接口都返回做了什么和最新状态 */
export interface Result { notes: string[]; state: State }

declare global { interface Window { HUB_TOKEN?: string; __TAURI_INTERNALS__?: unknown } }
const TOKEN = window.HUB_TOKEN ?? "";
export const IN_APP = "__TAURI_INTERNALS__" in window;

async function call<T>(path: string, body?: unknown): Promise<T> {
  if (IN_APP) {
    const { invoke } = await import("@tauri-apps/api/core");
    try {
      return await invoke<T>("api", { name: path.replace(/^\/api\//, ""), body: body ?? null });
    } catch (e) {
      const err = e as { error?: string; state?: State } | string;
      if (typeof err === "string") throw new Error(err);
      throw Object.assign(new Error(err.error || "请求失败"), { state: err.state });
    }
  }
  const init: RequestInit = { headers: { "X-Hub-Token": TOKEN } };
  if (body !== undefined) {
    init.method = "POST";
    init.headers = { ...init.headers, "Content-Type": "application/json" };
    init.body = JSON.stringify(body);
  }
  const r = await fetch(path, init);
  let data: any = {};
  try { data = await r.json(); } catch { /* 服务没返回 JSON */ }
  if (!r.ok) throw Object.assign(new Error(data.error || `请求失败（${r.status}）`), { state: data.state as State | undefined });
  return data as T;
}

export const api = {
  state: () => call<State>("/api/state"),
  check: () => call<Result>("/api/check", {}),
  updateAll: () => call<Result>("/api/update", {}),
  sync: (key: string) => call<Result>("/api/sync", { key }),
  install: (key: string, apps: AppKey[]) => call<Result>("/api/install", { key, apps }),
  /** 收编本机原有的独立技能：挪进 ~/.yuwanplugins/skills 统一存放，原位置换成链接 */
  adopt: (keys: string[]) => call<Result>("/api/adopt", { keys }),
  /** 在 skills.sh 上搜技能 */
  search: (q: string) => call<{ skills: WebSkill[] }>("/api/search", { q }),
  uninstall: (key: string, target?: { id: string; app: AppKey }) => call<Result>("/api/uninstall", { key, ...target }),
  verify: (key: string, app?: AppKey) => call<Result>("/api/verify", { key, app }),
  /** 只看 Codex 的技能清单超没超上限 */
  budget: () => call<Result>("/api/budget", {}),
  probe: (repo: string) => call<Probe>("/api/probe", { repo }),
  /** extra.path：插件在仓库里的子目录；extra.skills：只装这些技能（仓库里的文件夹） */
  add: (repo: string, branch: string, apps: AppKey[], extra?: { path?: string; skills?: string[] }) => call<Result>("/api/add", { repo, branch, apps, ...extra }),
  auto: (enabled: boolean, interval_minutes: number) => call<Result>("/api/auto", { enabled, interval_minutes }),
  guard: (enabled: boolean) => call<Result>("/api/guard", { enabled }),
  lock: (plugin: string, locked: boolean) => call<Result>("/api/lock", { plugin, locked }),
  save: (plugin: string) => call<Result>("/api/save", { plugin }),
  open: (target: string) => call<Result>("/api/open", { target }),
};

export function when(value: string | number | null | undefined): string {
  if (!value) return "—";
  const d = typeof value === "number" ? new Date(value * 1000) : new Date(value);
  if (isNaN(d.getTime())) return String(value);
  const s = (Date.now() - d.getTime()) / 1000;
  if (s < 60) return "刚刚";
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)} 天前`;
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

export const short = (sha?: string | null) => (sha ? sha.slice(0, 7) : "—");
export const isSkill = (p: Plugin) => p.kind === "skill";
/** 插件中心管的：受管插件，或者统一存放的 skill */
export const isOurs = (p: Plugin) => !!p.managed || !!p.skill?.ours;
export const installedApps = (p: Plugin) => p.apps.filter((a) => a.installed);
export const canDelete = (p: Plugin) => !p.official && (isOurs(p) || p.apps.some((a) => a.installed && !a.official));
/** 独立的 skill 没有远端可拉，只有统一存放的才能同步（补链接）；受管插件总能同步（拉取、补链接） */
/** 本机原有、还不归插件中心管的独立技能，能收编的：有真实的文件夹（指向别处的链接插件中心不动）*/
export const canAdopt = (p: Plugin) => isSkill(p) && !p.official && !p.skill?.ours && p.apps.some((a) => a.installed && !a.official && !a.linked);
export const canSync = (p: Plugin) => (isSkill(p) ? !!p.skill?.ours : !p.official && (!!p.managed || p.apps.some((a) => a.installed)));
/** token 数：粗估的，取整到十位 */
export const tokens = (n: number) => (n < 100 ? String(n) : (Math.round(n / 10) * 10).toLocaleString("en-US"));
/** 装着的 app 里，配置有问题或真实检查没通过的；插件中心管的 skill，链接被删了也算 */
export const troubledApps = (p: Plugin) => p.apps.filter((a) => (a.installed || (isSkill(p) && a.state === "missing"))
  && ((a.problems?.length ?? 0) > 0 || a.verify?.ok === false));
