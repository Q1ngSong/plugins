import { AlertTriangle, Library, Lock, Plus, Puzzle, ScrollText, ShieldCheck, Wrench } from "lucide-react";
import { Button } from "@/components/ui/button";
import { AppChip, Spinner, Tag } from "@/components/common/bits";
import { PluginActions } from "@/components/common/PluginActions";
import { api, APP_NAME, AppKey, installedApps, isOurs, isSkill, Kind, Plugin, State, tokens, troubledApps, when } from "@/lib/api";
import type { Run } from "@/lib/useRun";
import type { ConfirmFn } from "@/App";
import { cn } from "@/lib/utils";

export type Filter = "all" | AppKey;
export type KindFilter = "all" | Kind;

function iconTone(p: Plugin) {
  if (isOurs(p)) return "border-blue-100 bg-blue-50 text-blue-500 dark:border-blue-500/20 dark:bg-blue-500/10";
  if (p.official) return "border-slate-200 bg-slate-100 text-slate-500 dark:border-slate-500/20 dark:bg-slate-500/10 dark:text-slate-300";
  return "border-violet-100 bg-violet-50 text-violet-500 dark:border-violet-500/20 dark:bg-violet-500/10";
}

function Notices({ state, busy, run }: { state: State; busy: string | null; run: Run }) {
  const notes: string[] = [];
  if (!state.tools.claude) notes.push("没找到 Claude Code 的命令行 claude.exe，Claude Code 这边无法安装和更新。");
  if (!state.tools.codex) notes.push("没找到 Codex 的命令行 codex.exe，Codex 这边无法安装和更新。");
  if (!state.tools.git) notes.push("没找到 git，无法拉取插件。");
  const auto = state.hub.auto;
  if ((auto.enabled || state.hub.guard.enabled) && !auto.installed) notes.push("后台检查或自动更新是开着的，但后台任务没在运行。到设置里把它关掉再打开一次。");
  // 后台检查最近一天修过的配置
  const day = Date.now() - 86400_000;
  const repairs = state.plugins.flatMap((p) => p.apps.filter((a) => a.repair && new Date(a.repair.at).getTime() > day)
    .map((a) => ({ p, a, at: a.repair!.at })));
  const hasOurs = state.plugins.some((p) => isOurs(p) || p.apps.some((a) => a.port));
  return <>
    {notes.map((n) => (
      <div key={n} className="flex items-center gap-2.5 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3.5 py-2.5 text-[13px] text-amber-900 dark:text-amber-200">
        <AlertTriangle className="h-4 w-4 flex-none" /><span>{n}</span>
      </div>
    ))}
    {!state.hub.guard.enabled && hasOurs && (
      <div className="flex flex-wrap items-center gap-2.5 rounded-lg border border-blue-500/25 bg-blue-500/[.07] px-3.5 py-2 text-[13px] text-blue-900 dark:text-blue-200">
        <ShieldCheck className="h-4 w-4 flex-none" />
        <span className="min-w-0 flex-1">后台检查没开：别的程序（比如切换服务商的工具）改掉插件配置时，不会自动发现和修复。</span>
        <Button size="sm" disabled={!!busy} onClick={() => run("guard", () => api.guard(true), "已开启后台检查")}>
          {busy === "guard" && <Spinner />}打开后台检查
        </Button>
      </div>
    )}
    {repairs.length > 0 && (
      <div className="flex items-center gap-2.5 rounded-lg border border-sky-500/25 bg-sky-500/10 px-3.5 py-2 text-[13px] text-sky-900 dark:text-sky-200">
        <Wrench className="h-4 w-4 flex-none" />
        <span>后台检查最近一天修复了 {repairs.length} 处被改掉的插件配置（{repairs.map((x) => `${x.p.name} · ${APP_NAME[x.a.app]}`).join("、")}），最近一次在 {when(repairs.map((x) => x.at).sort().pop())}。</span>
      </div>
    )}
    <LibraryStrip state={state} busy={busy} run={run} />
  </>;
}

/* 技能库：按需的省下了多少、两边装好没有；没有按需的时候，说一句常驻的占了多少 */
function LibraryStrip({ state, busy, run }: { state: State; busy: string | null; run: Run }) {
  const lib = state.library;
  const resident = state.plugins.filter((p) => isOurs(p) && !p.on_demand).reduce((n, p) => n + (p.cost ?? 0), 0);
  if (!lib.on && !resident) return null;
  const problems = lib.apps.flatMap((a) => [...a.problems, ...(a.verify && !a.verify.ok ? [a.verify.detail] : [])].map((x) => `${APP_NAME[a.app]}：${x}`));
  const text = lib.on
    ? `技能库：按需的 ${lib.skills} 个技能原本每次会话要约 ${tokens(lib.saved)} token，现在只加载技能库的一句说明，约 ${tokens(lib.cost)} token。`
      + (resident ? `其余常驻的约 ${tokens(resident)} token。` : "")
    : `插件中心管的插件和技能都是常驻的，每次会话约占 ${tokens(resident)} token。不常用的可以在详情页改成按需，收进技能库，用到时再读。`;
  return (
    <div className={cn("flex flex-wrap items-center gap-2.5 rounded-lg border px-3.5 py-2 text-[13px]",
      problems.length ? "border-amber-500/30 bg-amber-500/10 text-amber-900 dark:text-amber-200" : "bg-card text-muted-foreground")}>
      <Library className="h-4 w-4 flex-none" />
      <span className="min-w-0 flex-1">{text}{problems.length > 0 && <> {problems.join("；")}</>}</span>
      {lib.on && (problems.length ? (
        <Button size="sm" disabled={!!busy} onClick={() => run("library", () => api.sync("library"), "技能库修好了")}>
          {busy === "library" && <Spinner />}修复
        </Button>
      ) : (
        <button className="text-xs font-medium text-blue-500 hover:underline disabled:opacity-50" disabled={!!busy}
          title="Codex 看模型提示里有没有技能库、按需的技能是不是真的不在提示里了；Claude Code 核对链接和读取许可"
          onClick={() => run("library", () => api.verify("library"), "检查完了")}>
          {busy === "library" ? <span className="inline-flex items-center gap-1"><Spinner />检查中…</span> : "检查技能库"}
        </button>
      ))}
    </div>
  );
}

export function HomeView({ state, filter, kind, busy, run, confirm, onOpen, onAdd, onInstall }: {
  state: State; filter: Filter; kind: KindFilter; busy: string | null; run: Run; confirm: ConfirmFn; onOpen: (key: string) => void; onAdd: () => void; onInstall: (key: string) => void;
}) {
  // 按需的哪边都没装，但两边都能从技能库读到，按 app 筛选时也列出来
  const list = state.plugins.filter((p) => (kind === "all" || (p.kind ?? "plugin") === kind)
    && (filter === "all" || p.on_demand || p.apps.some((a) => a.app === filter && a.installed)));
  const pending = state.plugins.filter((p) => p.managed?.needs_update).length;
  const what = kind === "skill" ? "技能" : "插件";

  return (
    <div className="flex flex-col gap-3">
      <Notices state={state} busy={busy} run={run} />
      {pending > 0 && (
        <div className="flex items-center gap-2.5 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3.5 py-2 text-[13px] text-amber-900 dark:text-amber-200">
          <span>{pending} 个插件有新版本</span><span className="flex-1" />
          <Button size="sm" disabled={!!busy} onClick={() => run("update-all", api.updateAll, "都已更新")}>
            {busy === "update-all" && <Spinner />}全部更新
          </Button>
        </div>
      )}

      {list.length === 0 ? (
        <div className="rounded-xl border-[1.5px] border-dashed p-10 text-center">
          <div className="mx-auto mb-2.5 flex h-14 w-14 items-center justify-center rounded-full bg-muted text-muted-foreground">
            {kind === "skill" ? <ScrollText className="h-6 w-6" /> : <Puzzle className="h-6 w-6" />}
          </div>
          <div className="text-base font-semibold">{filter === "all" ? `本机还没有${what}` : `${APP_NAME[filter]} 里还没有${what}`}</div>
          {kind === "skill" ? (
            <div className="mt-1 text-sm text-muted-foreground">独立的技能放在 ~/.claude/skills 或 ~/.codex/skills 里，放进去就会出现在这里</div>
          ) : (
            <>
              <div className="mt-1 text-sm text-muted-foreground">点右上角橙色 + 添加一个插件仓库</div>
              <Button className="mt-4" onClick={onAdd}><Plus className="h-4 w-4" />添加插件</Button>
            </>
          )}
        </div>
      ) : (
        // 瀑布流：卡片窄一些、一行放几张，高度跟着介绍长短走
        <div className="columns-[200px] gap-3">
          {list.map((p) => {
            const version = p.version || p.apps[0]?.version || "";
            const disabled = p.apps.some((a) => a.installed && a.enabled === false);
            const locked = !!p.managed?.locked, modified = !!p.managed?.modified.length;
            const troubled = troubledApps(p).length > 0;
            return (
              <div key={p.key} role="link" tabIndex={0} onClick={() => onOpen(p.key)}
                onKeyDown={(e) => { if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); onOpen(p.key); } }}
                className={cn("group relative mb-3 flex cursor-pointer break-inside-avoid flex-col gap-2.5 overflow-hidden rounded-2xl border bg-card p-4 transition-all hover:-translate-y-0.5 hover:border-blue-500/50 hover:shadow-md focus-visible:border-blue-500",
                  isOurs(p) && "before:pointer-events-none before:absolute before:inset-0 before:bg-gradient-to-b before:from-blue-500/[.07] before:to-transparent before:to-50%")}>
                <div className="relative flex items-start justify-between gap-2">
                  <div className={cn("flex h-9 w-9 flex-none items-center justify-center rounded-xl border transition-transform group-hover:scale-105", iconTone(p))}
                    title={isSkill(p) ? "技能" : "插件"}>
                    {isSkill(p) ? <ScrollText className="h-4 w-4" /> : <Puzzle className="h-4 w-4" />}
                  </div>
                  <PluginActions p={p} busy={busy} run={run} confirm={confirm} onInstall={() => onInstall(p.key)} />
                </div>
                <div className="relative">
                  <div className="break-all text-[15px] font-semibold leading-snug">{p.name}</div>
                  {(version || p.managed?.needs_update || p.managed?.error || disabled || locked || modified || troubled || p.on_demand || (kind === "all" && isSkill(p))) && (
                    <div className="mt-1.5 flex flex-wrap gap-1.5">
                      {kind === "all" && isSkill(p) && <Tag tone="sky">技能</Tag>}
                      {p.on_demand && <Tag tone="sky"><Library className="h-3 w-3" />按需</Tag>}
                      {version && <Tag tone="slate" mono>{version}</Tag>}
                      {locked && <Tag tone="slate"><Lock className="h-3 w-3" />已锁定</Tag>}
                      {modified && <Tag tone="amber">有本地修改</Tag>}
                      {p.managed?.needs_update && !p.managed.error && !troubled && <Tag tone="amber">有新版本</Tag>}
                      {troubled && <Tag tone="red">要检查</Tag>}
                      {p.managed?.error && !modified && <Tag tone="red">出错</Tag>}
                      {disabled && <Tag tone="red">已停用</Tag>}
                    </div>
                  )}
                </div>
                <p className="relative line-clamp-[8] text-xs leading-relaxed text-muted-foreground" title={p.description}>{p.description || (isSkill(p) ? "SKILL.md 里没有写介绍" : "插件里没有写介绍")}</p>
                <div className="relative flex flex-wrap gap-1.5">
                  {installedApps(p).map((a) => <AppChip key={a.app + a.id} app={a.app} />)}
                  {!installedApps(p).length && <span className="text-xs text-muted-foreground">{p.on_demand ? "在技能库里，用到时再读" : "哪边都没装"}</span>}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
