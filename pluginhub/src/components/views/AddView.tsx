import { useEffect, useMemo, useState } from "react";
import { Check, GitBranch, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { AppChip, Spinner, Tag } from "@/components/common/bits";
import { AppPicks } from "@/components/common/AppPicks";
import { api, APP_NAME, APPS, AppKey, Branch, Probe, ProbeSkill, short, when } from "@/lib/api";
import type { Run } from "@/lib/useRun";
import { cn } from "@/lib/utils";

const supports = (b: Branch) => APPS.filter((x) => b[x]);
/* 装成插件，还是只装仓库里的技能 */
type Mode = "plugin" | "skills";

/* 这个分支上能不能按这种方式装 */
const usable = (b: Branch, m: Mode) => (m === "skills" ? b.skills_total > 0 : b.claude || b.codex);
/* 插件源里列了好几个插件、链接又没指到其中一个：要挑一个，或者全部 */
const choices = (b?: Branch) => (b && !b.plugin_path ? b.plugins.filter((x) => x.path !== ".") : []);
const mustChoose = (b?: Branch) => choices(b).length > 1;

const LOCAL: Record<string, string> = { claude: "Claude Code 里已有同名的", codex: "Codex 里已有同名的", agents: "~/.agents/skills 里已有同名的（Codex 会读）" };

/* 输入里点名的技能（名字或文件夹名，"*" 是全部）在这个分支上对应的路径；链接直接指到一个技能的，就是那个 */
function preselect(b: Branch | undefined, probe: Probe): string[] {
  if (!b) return [];
  const free = b.skills.filter((s) => !probe.managed_skills.includes(s.path));
  const wanted = probe.input.skills.map((w) => w.toLowerCase());
  if (wanted.includes("*")) return free.map((s) => s.path);
  if (wanted.length) return free.filter((s) => wanted.includes(s.name.toLowerCase()) || wanted.includes((s.path.split("/").pop() || "").toLowerCase())).map((s) => s.path);
  if (probe.path && b.at_path?.skill) return free.filter((s) => s.path === probe.path).map((s) => s.path);
  return [];
}

export function AddView({ initialRepo, busy, run, onDone }: { initialRepo?: string; busy: string | null; run: Run; onDone: (key: string) => void }) {
  const [input, setInput] = useState(initialRepo ?? "");
  const [probe, setProbe] = useState<Probe | null>(null);
  const [probing, setProbing] = useState(false);
  const [error, setError] = useState("");
  const [mode, setMode] = useState<Mode>("plugin");
  const [branch, setBranch] = useState("");
  const [apps, setApps] = useState<AppKey[]>([]);
  const [picks, setPicks] = useState<string[]>([]);
  const [filter, setFilter] = useState("");
  /* 插件源里有好几个插件时挑的那个：null 还没挑，"" 是全部，其余是插件的子目录 */
  const [one, setOne] = useState<string | null>(null);

  const chosen = probe?.branches.find((b) => b.name === branch);
  const plug = chosen && one ? chosen.plugins.find((x) => x.path === one) : undefined;
  /* 装成插件时能装到哪些 app：挑了一个插件就看它的文件夹里有哪边的清单 */
  const pluginApps: AppKey[] = !chosen ? [] : plug ? APPS.filter((x) => x === "claude" || plug.codex) : supports(chosen);

  /* 定下方式和分支，顺带选好技能和 app */
  const settle = (r: Probe, m: Mode, b?: Branch) => {
    setMode(m); setBranch(b?.name ?? ""); setFilter(""); setOne(null);
    if (m === "skills") { setPicks(preselect(b, r)); setApps(r.input.agents.length ? r.input.agents : [...APPS]); }
    else { setPicks([]); setApps(b && !mustChoose(b) ? supports(b) : []); }
  };
  const query = async () => {
    const text = input.trim();
    if (!text || probing) return;
    setProbing(true); setError(""); setProbe(null); setBranch(""); setPicks([]);
    try {
      const r = await api.probe(text);
      setProbe(r);
      const m: Mode = r.mode === "skills" ? "skills" : "plugin";
      // 已经在管理的仓库只有一份克隆，只能接着用它跟的那个分支
      const want = r.managed_branch ?? r.ref;
      const b = r.branches.find((x) => x.name === want && usable(x, m)) ?? (r.managed_branch ? undefined : r.branches.find((x) => usable(x, m))) ?? r.branches.find((x) => x.name === want) ?? r.branches[0];
      settle(r, m, b);
    } catch (e) { setError((e as Error).message); }
    finally { setProbing(false); }
  };
  useEffect(() => { if (initialRepo) query(); }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const switchMode = (m: Mode) => {
    if (!probe || m === mode) return;
    settle(probe, m, chosen && usable(chosen, m) ? chosen : probe.branches.find((x) => usable(x, m) && (!probe.managed_branch || x.name === probe.managed_branch)));
  };
  const toggle = (path: string) => setPicks((p) => (p.includes(path) ? p.filter((x) => x !== path) : [...p, path]));
  const pickOne = (path: string) => {
    if (!chosen) return;
    setOne(path);
    const x = chosen.plugins.find((y) => y.path === path);
    setApps(x ? APPS.filter((a) => a === "claude" || x.codex) : supports(chosen));
  };

  const skills = chosen?.skills ?? [];
  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return q ? skills.filter((s) => `${s.name} ${s.path} ${s.description}`.toLowerCase().includes(q)) : skills;
  }, [skills, filter]);
  const installed = (s: ProbeSkill) => !!probe?.managed_skills.includes(s.path);
  /* 选中的 app 里，哪些已经看得到同名技能（Codex 还会读 ~/.agents/skills） */
  const clash = (s: ProbeSkill) => apps.filter((a) => s.local.includes(a) || (a === "codex" && s.local.includes("agents")));
  const clashes = mode === "skills" ? skills.filter((s) => picks.includes(s.path) && clash(s).length > 0) : [];
  const both = !!chosen && (chosen.claude || chosen.codex) && chosen.skills_total > 0;
  const blocked = mode === "plugin" && probe?.managed_as === "plugin";
  const canAdd = !!probe && !!chosen && apps.length > 0 && !blocked
    && (mode === "plugin" ? usable(chosen, "plugin") && (!mustChoose(chosen) || one !== null) : picks.length > 0 && !clashes.length);
  const notes = probe ? [...probe.input.notes, ...probe.notes] : [];

  const add = async () => {
    if (!probe || !chosen) return;
    if (mode === "skills") {
      const first = skills.find((s) => picks.includes(s.path));
      if (await run("add", () => api.add(probe.repo, chosen.name, apps, { skills: picks }), "已添加")) onDone(first ? `skill:${first.name}` : probe.id);
      return;
    }
    const path = one || chosen.plugin_path || undefined;
    if (await run("add", () => api.add(probe.repo, chosen.name, apps, path ? { path } : undefined), "已添加")) onDone(probe.id);
  };

  return (
    <div className="flex flex-col gap-4">
      <section className="rounded-xl border bg-card p-4">
        <div className="mb-2 text-sm font-semibold">仓库地址或命令</div>
        <form className="flex gap-2" onSubmit={(e) => { e.preventDefault(); query(); }}>
          <Input autoFocus value={input} onChange={(e) => { setInput(e.target.value); setProbe(null); setError(""); }}
            placeholder="GitHub 链接或 owner/repo、某个技能文件夹的链接，或 npx skills add … 命令" />
          <Button type="submit" variant="outline" disabled={!input.trim() || probing}>{probing ? <Spinner /> : <Search className="h-4 w-4" />}查询</Button>
        </form>
        <p className="mt-2 text-xs text-muted-foreground">
          认得：GitHub 链接或 owner/repo（可带 #分支）、仓库里插件子目录或技能文件夹的链接、<code className="font-mono">npx skills add …</code> 命令、skills.sh 的页面链接，
          以及 claude / codex 的 <code className="font-mono">plugin marketplace add …</code> 命令。查询只下载各分支最新提交里的清单和 SKILL.md。
        </p>
        {error && <p className="mt-3 whitespace-pre-wrap rounded-lg bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-300">{error}</p>}
        {probe && (
          <div className="mt-3 flex flex-col gap-1 text-xs">
            <p className="text-muted-foreground">识别为：{probe.input.text}</p>
            {notes.map((n) => <p key={n} className="rounded-lg bg-amber-500/10 px-3 py-1.5 text-amber-900 dark:text-amber-200">{n}</p>)}
          </div>
        )}
      </section>

      {probe && (
        <section className="rounded-xl border bg-card p-4">
          <div className="mb-1 flex items-center justify-between gap-3">
            <div className="text-sm font-semibold">选择分支</div>
            {both && (
              <div className="flex rounded-lg border p-0.5 text-xs">
                {([["plugin", "装插件"], ["skills", "只装技能"]] as const).map(([m, label]) => (
                  <button key={m} type="button" onClick={() => switchMode(m)}
                    className={cn("rounded-md px-2.5 py-1", mode === m ? "bg-blue-500 text-white" : "text-muted-foreground hover:text-foreground")}>{label}</button>
                ))}
              </div>
            )}
          </div>
          {blocked ? (
            <p className="mb-2 rounded-lg bg-amber-500/10 px-3 py-2 text-xs text-amber-900 dark:text-amber-200">{probe.id} 已经装成插件在管理了（一个仓库只管一个插件），可以在它的详情页里安装到别的 app。</p>
          ) : probe.managed ? (
            <p className="mb-2 rounded-lg bg-sky-500/10 px-3 py-2 text-xs text-sky-900 dark:text-sky-200">
              这个仓库已经在管理（{probe.id}，跟着 {probe.managed_branch} 分支），接着用那份克隆。
              {mode === "skills" && probe.managed_as === "plugin" && " 它已经装成了插件，插件自带的技能两边都能用；再单独装，同一个技能会出现两次。"}
            </p>
          ) : <p className="mb-3 text-xs text-muted-foreground">以后会跟着这个分支自动更新</p>}
          {!probe.branches.length && <p className="text-sm text-muted-foreground">这个仓库没有分支。</p>}
          <div className="flex flex-col gap-2">
            {probe.branches.map((b) => {
              const ok = usable(b, mode);
              const on = b.name === branch;
              const dead = !ok || blocked || (!!probe.managed_branch && b.name !== probe.managed_branch);
              return (
                <button key={b.name} type="button" disabled={dead} onClick={() => settle(probe, mode, b)}
                  className={cn("flex items-start gap-3 rounded-xl border px-3.5 py-3 text-left transition-colors",
                    !dead && "hover:border-blue-500/50", on && !dead && "border-blue-500 bg-blue-500/5", dead && "cursor-not-allowed opacity-60")}>
                  <span className={cn("mt-1 flex h-4 w-4 flex-none items-center justify-center rounded-full border", on && !dead ? "border-blue-500" : "border-border")}>
                    {on && !dead && <i className="h-2 w-2 rounded-full bg-blue-500" />}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5 text-sm font-semibold">
                      <GitBranch className="h-3.5 w-3.5 text-muted-foreground" />{b.name}
                      {b.default && <Tag tone="sky">默认</Tag>}
                      {mode === "plugin" && b.version && !mustChoose(b) && <Tag tone="slate" mono>{b.version}</Tag>}
                      {mode === "plugin" && b.plugin_path && <Tag tone="slate" mono>{b.plugin_path}</Tag>}
                    </div>
                    <div className="mt-1 break-all text-xs text-muted-foreground">
                      <code className="rounded bg-muted px-1 font-mono">{short(b.sha)}</code> {b.subject} · {when(b.date)}
                    </div>
                    <div className="mt-1.5 flex flex-wrap gap-1.5">
                      {mode === "plugin"
                        ? (ok ? <>{supports(b).map((x) => <AppChip key={x} app={x} />)}{mustChoose(b) && <Tag tone="slate">{choices(b).length} 个插件</Tag>}</> : <Tag tone="slate">不是插件</Tag>)
                        : (ok ? <Tag tone="sky">{b.skills_total} 个技能</Tag> : <Tag tone="slate">没有技能</Tag>)}
                    </div>
                  </div>
                </button>
              );
            })}
          </div>
          {probe.branches_total > probe.branches.length && (
            <p className="mt-2 text-xs text-muted-foreground">
              仓库有 {probe.branches_total} 个分支，这里列了默认分支和最近更新的 {probe.branches.length} 个。要别的分支，在地址后面加 <code className="font-mono">#分支名</code> 再查询。
            </p>
          )}
        </section>
      )}

      {probe && mode === "plugin" && chosen && !blocked && mustChoose(chosen) && (
        <section className="rounded-xl border bg-card p-4">
          <div className="mb-1 text-sm font-semibold">选择插件</div>
          <p className="mb-3 text-xs text-muted-foreground">这个仓库的插件源里列了 {choices(chosen).length} 个插件。一个仓库只管一个插件，或者整个装上。</p>
          <div className="flex max-h-80 flex-col gap-1.5 overflow-y-auto pr-1">
            {[{ name: `全部 ${choices(chosen).length} 个`, path: "", description: "插件源里列出的都装进 Claude Code", version: "", codex: chosen.codex }, ...choices(chosen)].map((x) => {
              const on = one === x.path;
              return (
                <button key={x.path} type="button" onClick={() => pickOne(x.path)}
                  className={cn("flex items-start gap-2.5 rounded-lg border px-3 py-2 text-left transition-colors hover:border-blue-500/50", on && "border-blue-500 bg-blue-500/5")}>
                  <span className={cn("mt-1 flex h-4 w-4 flex-none items-center justify-center rounded-full border", on ? "border-blue-500" : "border-border")}>
                    {on && <i className="h-2 w-2 rounded-full bg-blue-500" />}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5 text-sm font-medium">
                      {x.name}
                      {x.version && <Tag tone="slate" mono>{x.version}</Tag>}
                      {x.path && <code className="text-[11px] text-muted-foreground">{x.path}</code>}
                      <AppChip app="claude" />{x.codex && <AppChip app="codex" />}
                    </div>
                    {x.description && <p className="mt-0.5 line-clamp-2 text-xs text-muted-foreground">{x.description}</p>}
                  </div>
                </button>
              );
            })}
          </div>
        </section>
      )}

      {probe && mode === "skills" && chosen && (
        <section className="rounded-xl border bg-card p-4">
          <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
            <div className="text-sm font-semibold">选择技能（已选 {picks.length} / {skills.length}）</div>
            <div className="flex items-center gap-3 text-xs">
              <button type="button" className="text-blue-500 hover:underline" onClick={() => setPicks(skills.filter((s) => !installed(s)).map((s) => s.path))}>全选</button>
              <button type="button" className="text-blue-500 hover:underline" onClick={() => setPicks([])}>清空</button>
            </div>
          </div>
          {skills.length > 8 && <Input className="mb-2 h-8 text-sm" placeholder="按名字或说明筛选" value={filter} onChange={(e) => setFilter(e.target.value)} />}
          {chosen.skills_total > skills.length && <p className="mb-2 text-xs text-muted-foreground">仓库里有 {chosen.skills_total} 个技能，这里只列了前 {skills.length} 个。</p>}
          {!skills.length && <p className="text-sm text-muted-foreground">这个分支上没有技能。</p>}
          <div className="flex max-h-96 flex-col gap-1.5 overflow-y-auto pr-1">
            {shown.map((s) => {
              const done = installed(s);
              const on = done || picks.includes(s.path);
              return (
                <button key={s.path} type="button" disabled={done} onClick={() => toggle(s.path)}
                  className={cn("flex items-start gap-2.5 rounded-lg border px-3 py-2 text-left transition-colors", !done && "hover:border-blue-500/50", on && !done && "border-blue-500 bg-blue-500/5", done && "opacity-60")}>
                  <span className={cn("mt-0.5 flex h-4 w-4 flex-none items-center justify-center rounded border", on ? "border-blue-500 bg-blue-500 text-white" : "border-border")}>
                    {on && <Check className="h-3 w-3" strokeWidth={3} />}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5 text-sm font-medium">
                      {s.name}
                      {s.version && <Tag tone="slate" mono>{s.version}</Tag>}
                      {done && <Tag tone="emerald">已装</Tag>}
                      <code className="text-[11px] text-muted-foreground">{s.path || "."}</code>
                    </div>
                    {s.description && <p className="mt-0.5 line-clamp-2 text-xs text-muted-foreground">{s.description}</p>}
                    {!done && s.local.length > 0 && <p className="mt-1 text-[11px] text-amber-700 dark:text-amber-300">{s.local.map((x) => LOCAL[x] ?? x).join("；")}</p>}
                  </div>
                </button>
              );
            })}
          </div>
        </section>
      )}

      {probe && chosen && !blocked && (mode === "skills" || (usable(chosen, "plugin") && (!mustChoose(chosen) || one !== null))) && (
        <section className="rounded-xl border bg-card p-4">
          <div className="mb-3 text-sm font-semibold">装到哪些 app</div>
          <AppPicks ok={mode === "skills" ? [...APPS] : pluginApps} value={apps} onChange={setApps} />
          <p className="mt-3 text-xs text-muted-foreground">
            {mode === "skills"
              ? <>仓库克隆到 ~/.yuwanplugins/{probe.id}，选中的技能在 ~/.yuwanplugins/skills 里放一个指向它的链接，选中的 app 再链接过去。以后跟着 {chosen.name} 分支更新。</>
              : <>插件会克隆到 ~/.yuwanplugins/{probe.id}{(one || chosen.plugin_path) ? <>，用其中的 <code className="font-mono">{one || chosen.plugin_path}</code></> : null}，选中的 app 链接到那里。</>}
          </p>
          {clashes.length > 0 && (
            <p className="mt-3 rounded-lg bg-amber-500/10 px-3 py-2 text-xs text-amber-900 dark:text-amber-200">
              {clashes.map((s) => `${s.name}（${clash(s).map((a) => APP_NAME[a]).join("、")}）`).join("、")} 已经有同名的技能了，再装一份会重复。取消对应的 app，或者去掉这些技能。
            </p>
          )}
          <div className="mt-4 flex justify-end">
            <Button onClick={add} disabled={!canAdd || !!busy}>{busy === "add" && <Spinner />}添加并安装</Button>
          </div>
        </section>
      )}
    </div>
  );
}
