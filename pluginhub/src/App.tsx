/* 视图切换与顶部栏，结构参考 AgentPulse / cc-switch 的 App.tsx */
import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronLeft, Lock, LockOpen, PackagePlus, Plus, RefreshCw, Settings } from "lucide-react";
import { Toaster } from "@/components/ui/sonner";
import { Button } from "@/components/ui/button";
import { IconButton, Spinner } from "@/components/common/bits";
import { PluginActions } from "@/components/common/PluginActions";
import { useConfirm } from "@/components/common/useConfirm";
import { Filter, HomeView, KindFilter } from "@/components/views/HomeView";
import { PluginView } from "@/components/views/PluginView";
import { InstallDialog } from "@/components/views/InstallDialog";
import { AddView } from "@/components/views/AddView";
import { SettingsView } from "@/components/views/SettingsView";
import { api, APP_NAME, APPS, AppKey } from "@/lib/api";
import { useRun } from "@/lib/useRun";
import { cn } from "@/lib/utils";

type View = { name: "home" } | { name: "plugin"; key: string } | { name: "add"; repo?: string } | { name: "settings" };

const KINDS: [KindFilter, string, string][] = [
  ["all", "全部", "插件和技能都显示"],
  ["plugin", "插件", "只看插件"],
  ["skill", "技能", "只看独立的技能：不属于任何插件、单独放在 skills 文件夹里的 skill"],
];

export default function App() {
  const [filter, setFilter] = useState<Filter>(() => {
    try { return (localStorage.getItem("hub.filter") as Filter) || "all"; } catch { return "all"; }
  });
  const [kind, setKind] = useState<KindFilter>(() => {
    try { return (localStorage.getItem("hub.kind") as KindFilter) || "all"; } catch { return "all"; }
  });
  const [view, setView] = useState<View>({ name: "home" });
  const [installing, setInstalling] = useState<{ key: string; apps: AppKey[] } | null>(null);
  const { data: state, error } = useQuery({ queryKey: ["state"], queryFn: api.state, refetchInterval: 60_000 });
  const { confirm, dialog } = useConfirm();
  const { busy, run } = useRun();

  useEffect(() => { try { localStorage.setItem("hub.filter", filter); } catch { /* 存不了就算了 */ } }, [filter]);
  useEffect(() => { try { localStorage.setItem("hub.kind", kind); } catch { /* 存不了就算了 */ } }, [kind]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || view.name === "home" || document.querySelector("[role=dialog]")) return;
      if (["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement?.tagName ?? "")) { (document.activeElement as HTMLElement).blur(); return; }
      setView({ name: "home" });
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  });

  if (error) return <div className="p-10 text-center text-sm text-red-500">无法读取状态：{String((error as Error).message ?? error)}<br /><span className="text-muted-foreground">重新打开插件中心试试。</span></div>;
  if (!state) return <div className="flex h-screen items-center justify-center gap-2 text-sm text-muted-foreground"><Spinner />正在读取…</div>;

  const plugin = view.name === "plugin" ? state.plugins.find((p) => p.key === view.key) : undefined;
  const current: View = view.name === "plugin" && !plugin ? { name: "home" } : view; // 插件被删掉后回首页
  const title = current.name === "plugin" ? plugin!.name : current.name === "add" ? "添加插件" : current.name === "settings" ? "设置" : "";

  return (
    <div className="min-h-screen bg-background">
      <header className="sticky top-0 z-20 flex h-16 items-center justify-between gap-3 border-b border-border/60 bg-background/80 px-6 backdrop-blur-md">
        {current.name === "home" ? (
          <>
            <div className="flex min-w-0 items-center gap-2">
              <span className="hidden whitespace-nowrap text-xl font-bold tracking-tight text-blue-500 md:inline">插件中心</span>
              <Button variant="ghost" size="icon" className="h-8 w-8" title="设置" onClick={() => setView({ name: "settings" })}><Settings className="h-5 w-5" /></Button>
              <div className="ml-1 inline-flex gap-1 rounded-xl bg-muted p-1">
                {KINDS.map(([k, label, tip]) => (
                  <button key={k} onClick={() => setKind(k)} title={tip}
                    className={cn("flex h-8 items-center whitespace-nowrap rounded-md px-3 text-[13px] font-medium transition-all", k === kind ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:bg-background/50")}>
                    {label}
                  </button>
                ))}
              </div>
            </div>
            <div className="flex items-center gap-2">
              <div className="inline-flex gap-1 rounded-xl bg-muted p-1">
                {(["all", ...APPS] as Filter[]).map((k) => {
                  const found = k === "all" || !!state.tools[k];
                  return (
                    <button key={k} onClick={() => setFilter(k)} title={k === "all" ? "两个 app 里的都显示" : found ? `只看装在 ${APP_NAME[k]} 里的` : `没找到 ${APP_NAME[k]}`}
                      className={cn("flex h-8 items-center gap-2 whitespace-nowrap rounded-md px-3 text-[13px] font-medium transition-all", k === filter ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:bg-background/50")}>
                      {k !== "all" && <span className={cn("h-[7px] w-[7px] rounded-full", !found ? "bg-gray-300" : k === "claude" ? "bg-[#c2552f]" : "bg-[#0b8a6a]")} />}
                      {k === "all" ? "所有 app" : APP_NAME[k]}
                    </button>
                  );
                })}
              </div>
              <div className="inline-flex gap-1 rounded-xl bg-muted p-1">
                <Button variant="ghost" size="icon" className="h-8 w-8" title="检查更新" disabled={!!busy} onClick={() => run("check", api.check, "检查完了")}>
                  {busy === "check" ? <Spinner /> : <RefreshCw className="h-4 w-4" />}
                </Button>
              </div>
              <button onClick={() => setView({ name: "add" })} title="添加插件"
                className="inline-flex h-8 w-8 items-center justify-center rounded-full bg-orange-500 text-white shadow-lg shadow-orange-500/30 transition hover:-translate-y-px hover:bg-orange-600"><Plus className="h-5 w-5" /></button>
            </div>
          </>
        ) : (
          <>
            <div className="flex min-w-0 items-center gap-3">
              <Button variant="outline" size="icon" className="h-9 w-9 rounded-lg" title="返回（Esc）" onClick={() => setView({ name: "home" })}><ChevronLeft className="h-4 w-4" /></Button>
              <h1 className="truncate text-lg font-semibold">{title}</h1>
            </div>
            {current.name === "plugin" && plugin && (
              <div className="flex items-center gap-2">
                {Object.keys(plugin.install).length > 0 && (
                  <Button size="sm" disabled={!plugin.installable.length || !!busy} title={plugin.installable.length ? "装到其他 app" : "装不到其他 app，原因见下方"}
                    onClick={() => setInstalling({ key: plugin.key, apps: plugin.installable })}><PackagePlus className="h-3.5 w-3.5" />安装</Button>
                )}
                {plugin.managed && (() => {
                  const m = plugin.managed;
                  return (
                    <IconButton title={m.locked ? "已锁定：不检查、不拉取更新。点一下解锁" : "锁定：不再检查和拉取更新，插件文件夹里的修改会保留"}
                      aria-label={m.locked ? "解锁" : "锁定"} disabled={!!busy}
                      className={cn(m.locked && "bg-amber-500/10 text-amber-600 hover:bg-amber-500/15 hover:text-amber-700 dark:text-amber-400")}
                      onClick={() => run(`lock:${m.id}`, () => api.lock(m.id, !m.locked), m.locked ? "已解锁" : "已锁定")}>
                      {busy === `lock:${m.id}` ? <Spinner /> : m.locked ? <Lock /> : <LockOpen />}
                    </IconButton>
                  );
                })()}
                <PluginActions p={plugin} busy={busy} run={run} confirm={confirm} withDelete={false} />
              </div>
            )}
          </>
        )}
      </header>

      <main className={cn("mx-auto px-6 pb-20 pt-6", current.name === "home" ? "max-w-[1200px]" : "max-w-[860px]")}>
        <div key={current.name + (current.name === "plugin" ? current.key : "")} className={current.name === "home" ? "animate-fade-in" : "animate-slide-in"}>
            {current.name === "home" && <HomeView state={state} filter={filter} kind={kind} busy={busy} run={run} confirm={confirm} onOpen={(key) => setView({ name: "plugin", key })} onAdd={() => setView({ name: "add" })}
              onInstall={(key) => { const p = state.plugins.find((x) => x.key === key); setView({ name: "plugin", key }); if (p) setInstalling({ key, apps: p.installable }); }} />}
            {current.name === "plugin" && plugin && <PluginView p={plugin} busy={busy} run={run} confirm={confirm} onAdopt={(repo) => setView({ name: "add", repo })}
              onInstall={(apps) => setInstalling({ key: plugin.key, apps })} />}
            {current.name === "add" && <AddView initialRepo={current.repo} busy={busy} run={run} onDone={(key) => setView({ name: "plugin", key })} />}
            {current.name === "settings" && <SettingsView state={state} busy={busy} run={run} />}
        </div>
      </main>
      {installing && plugin && installing.key === plugin.key && (
        <InstallDialog p={plugin} initial={installing.apps} busy={busy} run={run} onClose={() => setInstalling(null)} />
      )}
      {dialog}
      <Toaster />
    </div>
  );
}

export type ConfirmFn = ReturnType<typeof useConfirm>["confirm"];
