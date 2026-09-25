import { useEffect, useState } from "react";
import { GitBranch, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { AppChip, Spinner, Tag } from "@/components/common/bits";
import { AppPicks } from "@/components/common/AppPicks";
import { api, APPS, AppKey, Branch, Probe, short, when } from "@/lib/api";
import type { Run } from "@/lib/useRun";
import { cn } from "@/lib/utils";

const supports = (b: Branch) => APPS.filter((x) => b[x]);

export function AddView({ initialRepo, busy, run, onDone }: { initialRepo?: string; busy: string | null; run: Run; onDone: (key: string) => void }) {
  const [repo, setRepo] = useState(initialRepo ?? "");
  const [probe, setProbe] = useState<Probe | null>(null);
  const [probing, setProbing] = useState(false);
  const [error, setError] = useState("");
  const [branch, setBranch] = useState("");
  const [apps, setApps] = useState<AppKey[]>([]);

  const pick = (b?: Branch) => { setBranch(b?.name ?? ""); setApps(b ? supports(b) : []); };
  const query = async () => {
    if (!repo.trim() || probing) return;
    setProbing(true); setError(""); setProbe(null); pick();
    try {
      const r = await api.probe(repo.trim());
      setProbe(r);
      pick(r.branches.find((b) => b.claude || b.codex));
    } catch (e) { setError((e as Error).message); }
    finally { setProbing(false); }
  };
  useEffect(() => { if (initialRepo) query(); }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const chosen = probe?.branches.find((b) => b.name === branch);
  const add = async () => {
    if (!probe || !chosen) return;
    if (await run("add", () => api.add(probe.repo, chosen.name, apps), "已添加")) onDone(probe.id);
  };

  return (
    <div className="flex flex-col gap-4">
      <section className="rounded-xl border bg-card p-4">
        <div className="mb-2 text-sm font-semibold">Git 仓库</div>
        <form className="flex gap-2" onSubmit={(e) => { e.preventDefault(); query(); }}>
          <Input autoFocus value={repo} onChange={(e) => { setRepo(e.target.value); setProbe(null); setError(""); }}
            placeholder="https://github.com/owner/repo 或 owner/repo" />
          <Button type="submit" variant="outline" disabled={!repo.trim() || probing}>{probing ? <Spinner /> : <Search className="h-4 w-4" />}查询</Button>
        </form>
        <p className="mt-2 text-xs text-muted-foreground">查询只下载各分支最新提交里的清单文件，看有哪些分支、版本号是多少。</p>
        {error && <p className="mt-3 rounded-lg bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-300">{error}</p>}
      </section>

      {probe && (
        <section className="rounded-xl border bg-card p-4">
          <div className="mb-1 text-sm font-semibold">选择分支</div>
          {probe.managed ? (
            <p className="mb-2 rounded-lg bg-amber-500/10 px-3 py-2 text-xs text-amber-900 dark:text-amber-200">{probe.id} 已经在管理了，可以在它的详情页里安装到别的 app。</p>
          ) : <p className="mb-3 text-xs text-muted-foreground">以后会跟着这个分支自动更新</p>}
          {!probe.branches.length && <p className="text-sm text-muted-foreground">这个仓库没有分支。</p>}
          <div className="flex flex-col gap-2">
            {probe.branches.map((b) => {
              const ok = b.claude || b.codex;
              const on = b.name === branch;
              return (
                <button key={b.name} type="button" disabled={!ok || probe.managed} onClick={() => pick(b)}
                  className={cn("flex items-start gap-3 rounded-xl border px-3.5 py-3 text-left transition-colors",
                    ok && !probe.managed && "hover:border-blue-500/50", on && !probe.managed && "border-blue-500 bg-blue-500/5", (!ok || probe.managed) && "cursor-not-allowed opacity-60")}>
                  <span className={cn("mt-1 flex h-4 w-4 flex-none items-center justify-center rounded-full border", on && !probe.managed ? "border-blue-500" : "border-border")}>
                    {on && !probe.managed && <i className="h-2 w-2 rounded-full bg-blue-500" />}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5 text-sm font-semibold">
                      <GitBranch className="h-3.5 w-3.5 text-muted-foreground" />{b.name}
                      {b.default && <Tag tone="sky">默认</Tag>}
                      {b.version && <Tag tone="slate" mono>{b.version}</Tag>}
                    </div>
                    <div className="mt-1 break-all text-xs text-muted-foreground">
                      <code className="rounded bg-muted px-1 font-mono">{short(b.sha)}</code> {b.subject} · {when(b.date)}
                    </div>
                    <div className="mt-1.5 flex flex-wrap gap-1.5">
                      {ok ? supports(b).map((x) => <AppChip key={x} app={x} />) : <Tag tone="slate">不是插件</Tag>}
                    </div>
                  </div>
                </button>
              );
            })}
          </div>
        </section>
      )}

      {probe && !probe.managed && chosen && (
        <section className="rounded-xl border bg-card p-4">
          <div className="mb-3 text-sm font-semibold">装到哪些 app</div>
          <AppPicks ok={supports(chosen)} value={apps} onChange={setApps} />
          <p className="mt-3 text-xs text-muted-foreground">插件会克隆到 ~/.yuwanplugins，选中的 app 链接到那里。</p>
          <div className="mt-4 flex justify-end">
            <Button onClick={add} disabled={!apps.length || !!busy}>{busy === "add" && <Spinner />}添加并安装</Button>
          </div>
        </section>
      )}
    </div>
  );
}
