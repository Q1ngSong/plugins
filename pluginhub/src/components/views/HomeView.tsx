import { useEffect, useState } from "react";
import { AlertTriangle, FolderInput, Globe, Lock, Plus, Puzzle, Scissors, ScrollText, Search, ShieldCheck, Wrench } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { AppChip, Spinner, Tag } from "@/components/common/bits";
import { PluginActions } from "@/components/common/PluginActions";
import { api, APP_NAME, AppKey, canAdopt, exeName, installedApps, isOurs, isSkill, Kind, Plugin, State, tokens, troubledApps, WebSkill, when } from "@/lib/api";
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
  if (!state.tools.claude) notes.push(`没找到 Claude Code 的命令行 ${exeName(state, "claude")}，Claude Code 这边无法安装和更新。`);
  if (!state.tools.codex) notes.push(`没找到 Codex 的命令行 ${exeName(state, "codex")}，Codex 这边无法安装和更新。`);
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
    <CodexSkills state={state} busy={busy} run={run} />
  </>;
}

/* Codex 的技能清单超了上限：它不报错，只是把说明截短，模型就挑不准技能。列出 Codex 里占得多的几个，删掉用不上的就好 */
function CodexSkills({ state, busy, run }: { state: State; busy: string | null; run: Run }) {
  const b = state.codex_skills;
  if (!b || b.ok) return null;
  const big = state.plugins.filter((p) => !p.official && (p.cost ?? 0) > 0 && p.apps.some((a) => a.app === "codex" && a.installed))
    .sort((x, y) => (y.cost ?? 0) - (x.cost ?? 0)).slice(0, 3);
  return (
    <div className="flex flex-wrap items-center gap-2.5 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3.5 py-2 text-[13px] text-amber-900 dark:text-amber-200">
      <Scissors className="h-4 w-4 flex-none" />
      <span className="min-w-0 flex-1">
        Codex 的技能清单{b.detail}。模型是看着说明挑技能的，说明不全就会挑不准。删掉 Codex 里用不上的插件或技能就好
        {big.length > 0 && `，占得多的有 ${big.map((p) => `${p.name}（约 ${tokens(p.cost ?? 0)} token）`).join("、")}`}。
      </span>
      <button className="text-xs font-medium hover:underline disabled:opacity-50" disabled={!!busy}
        title="运行 codex debug prompt-input，把模型提示里的每条说明和 SKILL.md 原文比一遍"
        onClick={() => run("budget", api.budget, "检查完了")}>
        {busy === "budget" ? <span className="inline-flex items-center gap-1"><Spinner />检查中…</span> : "重新检查"}
      </button>
    </div>
  );
}

/* 本机原有的技能不归插件中心管：提示一下，想管的话一键收编 */
function Unmanaged({ list, busy, run, confirm }: { list: Plugin[]; busy: string | null; run: Run; confirm: ConfirmFn }) {
  if (!list.length) return null;
  const adopt = async () => {
    const body = `把这 ${list.length} 个技能挪进 ~/.yuwanplugins/skills 统一存放，原来的位置换成链接。文件不会少，app 照常读到；之后后台检查会守着这些链接，装到另一个 app 也只要再放个链接。不想管的以后可以从所有 app 删除，文件夹会进备份。`;
    if (await confirm({ title: "交给插件中心管", body, okText: "全部收编" })) run("adopt-all", () => api.adopt(list.map((p) => p.key)), "收编完了");
  };
  return (
    <div className="flex flex-wrap items-center gap-2.5 rounded-lg border border-blue-500/25 bg-blue-500/[.07] px-3.5 py-2 text-[13px] text-blue-900 dark:text-blue-200">
      <FolderInput className="h-4 w-4 flex-none" />
      <span className="min-w-0 flex-1">本机有 {list.length} 个技能不归插件中心管：只列出来，不检查链接、不记来源。交给它管的话会统一存放，原位置换成链接，文件不动。</span>
      <Button size="sm" variant="outline" disabled={!!busy} onClick={adopt}>{busy === "adopt-all" && <Spinner />}全部收编</Button>
    </div>
  );
}

/* 本机没有或不够时，到 skills.sh 上搜：输入停下半秒才查，结果列在本机的下面 */
function WebResults({ query, local, onAdd }: { query: string; local: Plugin[]; onAdd: (repo: string) => void }) {
  const [state, setState] = useState<{ q: string; list: WebSkill[]; error: string; loading: boolean }>({ q: "", list: [], error: "", loading: false });
  const q = query.trim();
  useEffect(() => {
    if (q.length < 2) { setState({ q, list: [], error: "", loading: false }); return; }
    setState((s) => ({ ...s, loading: true }));
    const timer = setTimeout(async () => {
      try { const r = await api.search(q); setState({ q, list: r.skills, error: "", loading: false }); }
      catch (e) { setState({ q, list: [], error: (e as Error).message, loading: false }); }
    }, 500);
    return () => clearTimeout(timer);
  }, [q]);
  if (q.length < 2) return null;
  const have = new Set(local.map((p) => p.name.toLowerCase()));
  const open = (url: string) => api.open(url).catch((e) => toast.error("打不开", { description: String(e.message ?? e) }));
  return (
    <section className="mt-1">
      <div className="mb-2 flex items-center gap-2 text-xs font-semibold text-muted-foreground">
        <Globe className="h-3.5 w-3.5" />skills.sh 上的
        {state.loading && <Spinner />}
      </div>
      {state.error && <p className="text-xs text-muted-foreground">没搜到：{state.error}</p>}
      {!state.loading && !state.error && state.q === q && !state.list.length && <p className="text-xs text-muted-foreground">skills.sh 上没有匹配「{q}」的技能</p>}
      {state.list.length > 0 && (
        <div className="columns-[200px] gap-3">
          {state.list.map((s) => {
            const has = have.has(s.name.toLowerCase());
            return (
              <div key={s.url} className="mb-3 flex break-inside-avoid flex-col gap-2 rounded-2xl border border-dashed bg-card/60 p-4">
                <div className="flex items-start justify-between gap-2">
                  <div className="min-w-0">
                    <button type="button" title="在浏览器里打开它在 skills.sh 的页面，看说明再决定" onClick={() => open(s.url)}
                      className="break-all text-left text-[15px] font-semibold leading-snug hover:text-blue-500 hover:underline">{s.name}</button>
                    <button type="button" title="打开仓库的 GitHub 页面" onClick={() => open(`https://github.com/${s.source}`)}
                      className="mt-1 block break-all text-left text-xs text-muted-foreground hover:text-blue-500 hover:underline">{s.source}</button>
                  </div>
                  {has ? <Tag tone="emerald">本机已有</Tag> : (
                    <Button size="sm" variant="outline" className="flex-none" onClick={() => onAdd(s.url)}><Plus className="h-3.5 w-3.5" />添加</Button>
                  )}
                </div>
                <div className="text-xs text-muted-foreground">{s.installs.toLocaleString("en-US")} 次安装</div>
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

export function HomeView({ state, filter, kind, query, busy, run, confirm, onOpen, onAdd, onInstall }: {
  state: State; filter: Filter; kind: KindFilter; query: string; busy: string | null; run: Run; confirm: ConfirmFn; onOpen: (key: string) => void; onAdd: (repo?: string) => void; onInstall: (key: string) => void;
}) {
  // 搜索：每个词都要出现在名字、介绍、仓库地址或两边的插件名里
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const text = (p: Plugin) => [p.name, p.key, p.description, p.repo, ...p.apps.map((a) => a.id)].join("\n").toLowerCase();
  const list = state.plugins.filter((p) => (kind === "all" || (p.kind ?? "plugin") === kind)
    && (filter === "all" || p.apps.some((a) => a.app === filter && a.installed))
    && words.every((w) => text(p).includes(w)));
  const pending = state.plugins.filter((p) => p.managed?.needs_update).length;
  const what = kind === "skill" ? "技能" : kind === "plugin" ? "插件" : "插件或技能";
  const unmanaged = kind !== "plugin" && !words.length ? state.plugins.filter(canAdopt) : [];

  return (
    <div className="flex flex-col gap-3">
      <Notices state={state} busy={busy} run={run} />
      <Unmanaged list={unmanaged} busy={busy} run={run} confirm={confirm} />
      {pending > 0 && (
        <div className="flex items-center gap-2.5 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3.5 py-2 text-[13px] text-amber-900 dark:text-amber-200">
          <span>{pending} 个插件有新版本</span><span className="flex-1" />
          <Button size="sm" disabled={!!busy} onClick={() => run("update-all", api.updateAll, "都已更新")}>
            {busy === "update-all" && <Spinner />}全部更新
          </Button>
        </div>
      )}

      {list.length === 0 && words.length > 0 ? (
        <div className="rounded-xl border-[1.5px] border-dashed p-10 text-center">
          <div className="mx-auto mb-2.5 flex h-14 w-14 items-center justify-center rounded-full bg-muted text-muted-foreground"><Search className="h-6 w-6" /></div>
          <div className="text-base font-semibold">本机没有匹配「{query.trim()}」的{what}</div>
          <div className="mt-1 text-sm text-muted-foreground">按名字、介绍、仓库地址和两边的插件名搜，多个词用空格隔开</div>
        </div>
      ) : list.length === 0 ? (
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
              <Button className="mt-4" onClick={() => onAdd()}><Plus className="h-4 w-4" />添加插件</Button>
            </>
          )}
        </div>
      ) : (
        // 瀑布流：卡片窄一些、一行放几张，高度跟着介绍长短走。
        // 分栏布局会把超出栏框的部分裁掉，每张卡片外面套一层、顶上留 2 像素内边距，悬停上浮 2 像素时还在栏框里
        // （外边距不行：栏首的外边距会被分栏截掉）
        <div className="columns-[200px] gap-3">
          {list.map((p) => {
            const version = p.version || p.apps[0]?.version || "";
            const disabled = p.apps.some((a) => a.installed && a.enabled === false);
            const locked = !!p.managed?.locked, modified = !!p.managed?.modified.length;
            const troubled = troubledApps(p).length > 0;
            return (
              <div key={p.key} className="mb-3 break-inside-avoid pt-0.5">
              <div role="link" tabIndex={0} onClick={() => onOpen(p.key)}
                onKeyDown={(e) => { if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); onOpen(p.key); } }}
                className={cn("group relative flex cursor-pointer flex-col gap-2.5 overflow-hidden rounded-2xl border bg-card p-4 transition-all hover:-translate-y-0.5 hover:border-blue-500/50 hover:shadow-md focus-visible:border-blue-500",
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
                  {(version || p.managed?.needs_update || p.managed?.error || disabled || locked || modified || troubled || (kind === "all" && isSkill(p))) && (
                    <div className="mt-1.5 flex flex-wrap gap-1.5">
                      {kind === "all" && isSkill(p) && <Tag tone="sky">技能</Tag>}
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
                <p className="relative line-clamp-[8] text-xs leading-relaxed text-muted-foreground">{p.description || (isSkill(p) ? "SKILL.md 里没有写介绍" : "插件里没有写介绍")}</p>
                <div className="relative flex flex-wrap gap-1.5">
                  {installedApps(p).map((a) => <AppChip key={a.app + a.id} app={a.app} />)}
                  {!installedApps(p).length && <span className="text-xs text-muted-foreground">哪边都没装</span>}
                </div>
              </div>
              </div>
            );
          })}
        </div>
      )}
      {kind !== "plugin" && <WebResults query={query} local={state.plugins.filter(isSkill)} onAdd={onAdd} />}
    </div>
  );
}
