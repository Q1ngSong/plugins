import { useEffect, useState } from "react";
import { Check } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { AppChip, Spinner } from "@/components/common/bits";
import { api, APP_NAME, APPS, AppKey, howText, Plugin } from "@/lib/api";
import type { Run } from "@/lib/useRun";
import { cn } from "@/lib/utils";

/* 把插件装到其他 app：每个 app 一行，写明怎么装；装不了的写原因 */
export function InstallDialog({ p, initial, onClose, busy, run }: { p: Plugin; initial: AppKey[]; onClose: () => void; busy: string | null; run: Run }) {
  const [apps, setApps] = useState<AppKey[]>([]);
  useEffect(() => { setApps(initial.filter((x) => p.installable.includes(x))); }, [initial, p.installable]);
  const ported = apps.filter((x) => p.install[x]?.how === "port");

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>安装 {p.name}</DialogTitle>
          <DialogDescription>选要装到哪些 app</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-2.5 px-6 py-5">
          {APPS.map((x) => {
            const has = p.apps.some((a) => a.app === x && a.installed);
            const o = p.install[x];
            const can = !!o?.how;
            const on = has || apps.includes(x);
            return (
              <button key={x} type="button" disabled={!can}
                onClick={() => setApps(apps.includes(x) ? apps.filter((v) => v !== x) : [...apps, x])}
                className={cn("flex items-start gap-3 rounded-xl border px-3.5 py-3 text-left transition-colors",
                  can && "hover:border-blue-500/50", can && on && "border-blue-500 bg-blue-500/5", !can && "cursor-not-allowed opacity-70")}>
                <span className={cn("mt-0.5 flex h-4 w-4 flex-none items-center justify-center rounded border", on ? "border-blue-500 bg-blue-500 text-white" : "border-border")}>
                  {on && <Check className="h-3 w-3" strokeWidth={3} />}
                </span>
                <div className="min-w-0 flex-1">
                  <AppChip app={x} />
                  <div className="mt-1.5 text-xs text-muted-foreground">{has ? "已经装了" : o ? howText(o) : "—"}</div>
                </div>
              </button>
            );
          })}
          {ported.length > 0 && (
            <p className="rounded-lg bg-amber-500/10 px-3 py-2 text-xs leading-relaxed text-amber-900 dark:text-amber-200">
              移植只带 skill，钩子、MCP 服务和 App 集成两边格式不同，不会带过去。为 {ported.map((x) => APP_NAME[p.install[x]!.from!]).join("、")} 写的 skill 可能用到它专有的功能，在 {ported.map((x) => APP_NAME[x]).join("、")} 里不一定都能用。源插件更新后点同步会重新复制。
            </p>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={onClose}>取消</Button>
          <Button size="sm" disabled={!apps.length || !!busy}
            onClick={async () => { if (await run(`install:${p.key}`, () => api.install(p.key, apps), "已安装")) onClose(); }}>
            {busy === `install:${p.key}` && <Spinner />}安装
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
