import { CheckCircle2, FolderOpen, XCircle } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/common/bits";
import { api, exeName, State, taskKind, when } from "@/lib/api";
import type { Release } from "@/lib/release";
import type { Run } from "@/lib/useRun";

const INTERVALS: [number, string][] = [[15, "每 15 分钟"], [30, "每 30 分钟"], [60, "每小时"], [360, "每 6 小时"], [1440, "每天"]];

const Card = ({ title, children, desc }: { title: string; desc?: string; children: React.ReactNode }) => (
  <section className="rounded-xl border bg-card p-4">
    <div className="text-sm font-semibold">{title}</div>
    {desc && <div className="mt-0.5 text-xs text-muted-foreground">{desc}</div>}
    <div className="mt-3">{children}</div>
  </section>
);

export function SettingsView({ state, busy, run, openDirect, onOpenDirect, newVersion, onNewVersion }: {
  state: State; busy: string | null; run: Run;
  /** 点插件名时直接打开存放位置，不先问一下（弹窗里点「以后都直接打开」也会打开它） */
  openDirect: boolean; onOpenDirect: (v: boolean) => void;
  /** 插件中心自己的新版本：null 是问过 GitHub 了没有更新，undefined 是还没问到；点它看说明和下载 */
  newVersion: Release | null | undefined; onNewVersion: () => void;
}) {
  const auto = state.hub.auto;
  const on = !!(auto.enabled && auto.installed);
  const minutes = auto.interval_minutes || 60;
  const open = (target: string) => api.open(target).catch((e) => toast.error("打不开", { description: String(e.message ?? e) }));
  const tools: [string, string][] = [[`Claude Code（${exeName(state, "claude")}）`, state.tools.claude], [`Codex（${exeName(state, "codex")}）`, state.tools.codex], ["git", state.tools.git]];

  const guard = state.hub.guard;
  return (
    <div className="flex flex-col gap-3">
      <Card title="后台检查" desc="每天检查一次两边的插件配置有没有被别的程序改掉（比如切换服务商的工具重写了 config.toml 或 settings.json、链接被删），改掉了就自动修复，再做一遍真实检查，确认 app 真的加载了插件。">
        <div className="flex flex-wrap items-center gap-3">
          <Switch checked={guard.enabled} disabled={!!busy} onCheckedChange={(v) => run("guard", () => api.guard(v), v ? "已开启后台检查" : "已关闭后台检查")} />
          <span className="text-sm">{guard.enabled ? "已开启" : "已关闭"}</span>
          {busy === "guard" && <Spinner />}
        </div>
        <div className="mt-2 text-xs text-muted-foreground">
          {guard.enabled ? `${auto.installed ? `后台任务在运行 · 下次 ${auto.next_run || "—"}` : "后台任务没在运行，把开关关掉再打开一次"}` : "关闭时只在你点同步或检查更新时才会发现问题"}
          {guard.last && ` · 上次检查 ${when(guard.last)}`}
        </div>
      </Card>

      <Card title="自动更新" desc="按间隔拉取受管插件的新版本（锁定的和插件文件夹里有修改的不拉取）。开机登录后也会检查一次。">
        <div className="flex flex-wrap items-center gap-3">
          <Switch checked={on} disabled={!!busy} onCheckedChange={(v) => run("auto", () => api.auto(v, minutes), v ? "已开启自动更新" : "已关闭自动更新")} />
          <span className="text-sm">{on ? "已开启" : "已关闭"}</span>
          <select className="h-8 rounded-md border bg-background px-2 text-sm" value={minutes} disabled={!!busy}
            onChange={(e) => { if (on) run("auto", () => api.auto(true, +e.target.value), "已修改间隔"); else toast("先打开自动更新，再选间隔"); }}>
            {INTERVALS.map(([v, t]) => <option key={v} value={v}>{t}</option>)}
          </select>
          {busy === "auto" && <Spinner />}
        </div>
        <div className="mt-2 text-xs text-muted-foreground">
          {on ? `${auto.mode === "startup" ? "开机自启的后台进程" : taskKind(state)} · 下次检查 ${auto.next_run || "—"}` : "关闭时只在你点同步或检查更新时才更新"}
          {state.hub.last_auto && ` · 上次自动检查 ${when(state.hub.last_auto.at)}`}
        </div>
        <div className="mt-3 flex gap-2">
          <Button variant="outline" size="sm" disabled={!!busy} onClick={() => run("check", api.check, "检查完了")}>{busy === "check" && <Spinner />}现在检查</Button>
          <Button variant="outline" size="sm" disabled={!!busy} onClick={() => run("update-all", api.updateAll, "都已更新")}>{busy === "update-all" && <Spinner />}全部更新</Button>
        </div>
      </Card>

      <Card title="文件位置">
        <div className="flex flex-col gap-2 text-[13px]">
          {([["插件文件夹（两边都链接到这里，不要在这里改插件）", state.hub.plugins_dir, "plugins_dir"],
            ["另存的修改（另存并还原时存到这里）", state.hub.saved_dir, "saved_dir"],
            ["插件中心的配置、日志和备份", state.hub.home, "hub_dir"]] as const).map(([k, v, t]) => (
            <div key={t} className="flex items-center gap-3 rounded-lg bg-muted/50 px-3 py-2">
              <div className="min-w-0 flex-1">
                <div className="text-xs text-muted-foreground">{k}</div>
                <div className="break-all font-mono text-xs">{v}</div>
              </div>
              <Button variant="ghost" size="sm" onClick={() => open(t)}><FolderOpen className="h-3.5 w-3.5" />打开</Button>
            </div>
          ))}
          <div className="mt-1 flex items-center gap-3 border-t pt-3">
            <Switch checked={openDirect} onCheckedChange={onOpenDirect} />
            <div className="min-w-0">
              <div>在详情页点插件或技能的名字时，直接打开它的存放位置</div>
              <div className="text-xs text-muted-foreground">关闭时会先弹窗问一下；弹窗里点「以后都直接打开」也会把这个开关打开。</div>
            </div>
          </div>
        </div>
      </Card>

      <Card title="运行环境">
        <div className="flex flex-col gap-1.5 text-[13px]">
          {tools.map(([k, v]) => (
            <div key={k} className="flex items-start gap-2">
              {v ? <CheckCircle2 className="mt-0.5 h-4 w-4 flex-none text-emerald-500" /> : <XCircle className="mt-0.5 h-4 w-4 flex-none text-red-500" />}
              <span className="w-44 flex-none">{k}</span>
              <span className="min-w-0 break-all font-mono text-xs text-muted-foreground">{v || "没找到"}</span>
            </div>
          ))}
          <div className="mt-1.5 border-t pt-2 text-xs text-muted-foreground">
            后台任务运行的命令（就是这个程序自己）：
            <div className="mt-1 break-all font-mono text-[11px]">{state.hub.launcher}</div>
          </div>
        </div>
      </Card>

      <Card title="日志" desc="最新的在最上面">
        <pre className="max-h-72 overflow-auto whitespace-pre-wrap rounded-lg bg-muted/50 p-3 font-mono text-[11.5px] leading-relaxed text-muted-foreground">{state.log.join("\n") || "暂无记录"}</pre>
      </Card>

      <p className="text-center text-xs text-muted-foreground">
        插件中心 {state.hub.version}
        {newVersion && <> · <button type="button" className="font-medium text-orange-600 hover:underline dark:text-orange-400" onClick={onNewVersion}>有新版本 {newVersion.version}</button></>}
        {newVersion === null && "（已是最新）"}
        {" "}· 数据更新于 {when(state.generated_at)}
      </p>
    </div>
  );
}
