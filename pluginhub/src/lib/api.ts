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
}

/** 还没装的 app 能怎么装：native 直接装；adopt 先交给插件中心管理；port 移植 skill；null 装不了（why 是原因） */
export interface InstallOption { how: "native" | "adopt" | "port" | null; from?: AppKey; why?: string }

export interface Commit { sha: string; short: string; subject: string; date: string }

export interface Managed {
  id: string;
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

export interface Plugin {
  key: string;
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
}

export function howText(o: InstallOption): string {
  if (o.how === "native") return "直接安装";
  if (o.how === "adopt") return "克隆到 ~/.yuwanplugins 交给插件中心管理，再装进去";
  if (o.how === "port") return `从 ${APP_NAME[o.from ?? "codex"]} 移植 skill：复制一份到 ~/.yuwanplugins 再链接进去`;
  return o.why ?? "装不了";
}

export interface AutoUpdate { enabled: boolean; interval_minutes: number; installed: boolean; mode: string | null; next_run?: string }

export interface State {
  generated_at: string;
  hub: {
    version: string; home: string; plugins_dir: string; auto: AutoUpdate; last_auto: { at: string; notes: string[] } | null;
    /** 后台检查：定时看两边的插件配置有没有被改掉 */
    guard: { enabled: boolean; last: string | null };
    /** 后台任务实际运行的命令（装了桌面版就是它的 exe） */
    launcher: string;
  };
  tools: Record<AppKey | "git", string>;
  plugins: Plugin[];
  log: string[];
}

export interface Branch {
  name: string; sha: string; date: string; subject: string; default: boolean;
  version: string; title: string; description: string; claude: boolean; codex: boolean;
}
export interface Probe { repo: string; id: string; managed: boolean; branches: Branch[] }

/** 改动类接口都返回做了什么和最新状态 */
export interface Result { notes: string[]; state: State }

declare global { interface Window { HUB_TOKEN?: string; __TAURI_INTERNALS__?: unknown } }
const TOKEN = window.HUB_TOKEN ?? "";
const IN_APP = "__TAURI_INTERNALS__" in window;

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
  uninstall: (key: string, target?: { id: string; app: AppKey }) => call<Result>("/api/uninstall", { key, ...target }),
  verify: (key: string, app: AppKey) => call<Result>("/api/verify", { key, app }),
  probe: (repo: string) => call<Probe>("/api/probe", { repo }),
  add: (repo: string, branch: string, apps: AppKey[]) => call<Result>("/api/add", { repo, branch, apps }),
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
export const installedApps = (p: Plugin) => p.apps.filter((a) => a.installed);
export const canDelete = (p: Plugin) => !p.official && (!!p.managed || p.apps.some((a) => a.installed && !a.official));
export const canSync = (p: Plugin) => !p.official && p.apps.some((a) => a.installed);
/** 装着的 app 里，配置有问题或真实检查没通过的 */
export const troubledApps = (p: Plugin) => p.apps.filter((a) => a.installed && ((a.problems?.length ?? 0) > 0 || a.verify?.ok === false));
