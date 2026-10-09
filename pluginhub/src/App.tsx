/* 视图切换与顶部栏，结构参考 AgentPulse / cc-switch 的 App.tsx */
import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronLeft, CircleArrowUp, Lock, LockOpen, PackagePlus, Plus, RefreshCw, Search, Settings, X } from "lucide-react";
import { toast } from "sonner";
import { Toaster } from "@/components/ui/sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { IconButton, Spinner } from "@/components/common/bits";
import { PluginActions } from "@/components/common/PluginActions";
import { useConfirm } from "@/components/common/useConfirm";
import { Filter, HomeView, KindFilter } from "@/components/views/HomeView";
import { PluginView } from "@/components/views/PluginView";
import { InstallDialog } from "@/components/views/InstallDialog";
import { AddView } from "@/components/views/AddView";
import { SettingsView } from "@/components/views/SettingsView";
import { api, APP_NAME, APPS, AppKey, fileManager, isSkill, storageFolder, when } from "@/lib/api";
import { useHubUpdate } from "@/lib/update";
import { useRun } from "@/lib/useRun";
import { cn } from "@/lib/utils";

type View = { name: "home" } | { name: "plugin"; key: string } | { name: "add"; repo?: string } | { name: "settings" };

/** 顶栏标题点开的项目页 */
const PROJECT_URL = "https://github.com/Q1ngSong/plugins";

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
  // 点插件名打开存放位置前要不要先问一下：弹窗里点过「以后都直接打开」，或在设置里关掉询问，就不问了
  const [openDirect, setOpenDirect] = useState(() => {
    try { return localStorage.getItem("hub.openFolder") === "direct"; } catch { return false; }
  });
  const [view, setView] = useState<View>({ name: "home" });
  const [query, setQuery] = useState("");
  const [installing, setInstalling] = useState<{ key: string; apps: AppKey[] } | null>(null);
  const { data: state, error } = useQuery({ queryKey: ["state"], queryFn: api.state, refetchInterval: 60_000 });
  // 插件中心自己有没有新版本：打开时查一次，之后每 6 小时查一次
  const update = useHubUpdate(state?.hub.version ?? "");
  const { confirm, dialog } = useConfirm();
  const { busy, run } = useRun();
  const openProject = async () => {
    if (await confirm({ title: "打开项目主页？", body: `在浏览器里打开插件中心在 GitHub 上的项目页：\n${PROJECT_URL}`, okText: "打开" })) {
      api.open(PROJECT_URL).catch((e) => toast.error("打不开", { description: String(e.message ?? e) }));
    }
  };

  useEffect(() => { try { localStorage.setItem("hub.filter", filter); } catch { /* 存不了就算了 */ } }, [filter]);
  useEffect(() => { try { localStorage.setItem("hub.kind", kind); } catch { /* 存不了就算了 */ } }, [kind]);
  useEffect(() => { try { localStorage.setItem("hub.openFolder", openDirect ? "direct" : "ask"); } catch { /* 存不了就算了 */ } }, [openDirect]);
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

  const newVersion = update.phase === "found" ? update.found : null;
  const openNewVersion = async () => {
    if (!newVersion) return;
    const notes = newVersion.notes.trim();
    const body = [
      `当前 ${state.hub.version}，${newVersion.at ? `${when(newVersion.at)} 发布了` : "GitHub 上发布了"} ${newVersion.version}。`,
      notes.length > 1500 ? `${notes.slice(0, 1500)}…` : notes,
      update.canInstall ? "下载并安装新版本，装好后插件中心会自动重新打开；配置和插件都保留。" : `在浏览器里打开发布页，下载新的安装包：\n${newVersion.url}`,
    ].filter(Boolean).join("\n\n");
    if (await confirm({ title: `插件中心有新版本 ${newVersion.version}`, body, okText: update.canInstall ? "下载并安装" : "打开发布页" })) {
      update.install().catch((e) => toast.error("更新失败", { description: String(e.message ?? e) }));
    }
  };

  const plugin = view.name === "plugin" ? state.plugins.find((p) => p.key === view.key) : undefined;
  const current: View = view.name === "plugin" && !plugin ? { name: "home" } : view; // 插件被删掉后回首页
  const title = current.name === "plugin" ? plugin!.name : current.name === "add" ? "添加插件" : current.name === "settings" ? "设置" : "";
  const folder = current.name === "plugin" && plugin ? storageFolder(plugin) : null;
  const openFolder = async () => {
    if (!folder || !plugin) return;
    if (!openDirect) {
      const answer = await confirm({
        title: "打开存放位置？",
        body: `在${fileManager(state)}里打开${isSkill(plugin) ? "这个技能" : "这个插件"}存放的文件夹：\n${folder}`,
        okText: "打开", altText: "以后都直接打开",
      });
      if (!answer) return;
      if (answer === "alt") setOpenDirect(true);
    }
    api.open(folder).catch((e) => toast.error("打不开", { description: String(e.message ?? e) }));
  };

  return (
    <div className="min-h-screen bg-background">
      <header className="sticky top-0 z-20 flex h-16 items-center justify-between gap-3 border-b border-border/60 bg-background/80 px-6 backdrop-blur-md">
        {current.name === "home" ? (
          <>
            <div className="flex min-w-0 items-center gap-2">
              <button type="button" title="打开 GitHub 上的项目页" onClick={openProject}
                className="hidden whitespace-nowrap text-xl font-bold tracking-tight text-blue-500 transition hover:text-blue-600 md:inline">插件中心</button>
              {newVersion && (
                <button type="button" title={`插件中心有新版本 ${newVersion.version}，点一下看说明和下载`} onClick={openNewVersion}
                  className="inline-flex h-7 items-center gap-1 whitespace-nowrap rounded-full bg-orange-500/10 px-2.5 text-xs font-medium text-orange-600 transition hover:bg-orange-500/20 dark:text-orange-400">
                  <CircleArrowUp className="h-3.5 w-3.5" />有新版本 {newVersion.version}
                </button>
              )}
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
              <div className="relative">
                <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索插件或技能"
                  title="按名字、介绍、仓库地址和两边的插件名搜，多个词用空格隔开；Esc 清空"
                  className="h-8 w-40 pl-8 pr-7 md:w-52"
                  onKeyDown={(e) => { if (e.key === "Escape") { setQuery(""); e.currentTarget.blur(); } }} />
                {query && (
                  <button type="button" title="清空" onClick={() => setQuery("")}
                    className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"><X className="h-3.5 w-3.5" /></button>
                )}
              </div>
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
                <Button variant="ghost" size="icon" className="h-8 w-8" title="检查更新（插件的和插件中心自己的）" disabled={!!busy} onClick={() => { update.check(); run("check", api.check, "检查完了"); }}>
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
              {folder ? (
                <button type="button" title={`打开存放位置：${folder}`} onClick={openFolder}
                  className="truncate text-lg font-semibold transition hover:text-blue-600 hover:underline">{title}</button>
              ) : (
                <h1 className="truncate text-lg font-semibold">{title}</h1>
              )}
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
            {current.name === "home" && <HomeView state={state} filter={filter} kind={kind} query={query} busy={busy} run={run} confirm={confirm} onOpen={(key) => setView({ name: "plugin", key })} onAdd={(repo) => setView({ name: "add", repo })}
              onInstall={(key) => { const p = state.plugins.find((x) => x.key === key); setView({ name: "plugin", key }); if (p) setInstalling({ key, apps: p.installable }); }} />}
            {current.name === "plugin" && plugin && <PluginView p={plugin} busy={busy} run={run} confirm={confirm} onAdopt={(repo) => setView({ name: "add", repo })}
              onInstall={(apps) => setInstalling({ key: plugin.key, apps })} />}
            {current.name === "add" && <AddView initialRepo={current.repo} busy={busy} run={run} onDone={(key) => setView({ name: "plugin", key })} />}
            {current.name === "settings" && <SettingsView state={state} busy={busy} run={run} openDirect={openDirect} onOpenDirect={setOpenDirect} update={update} onNewVersion={openNewVersion} />}
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
