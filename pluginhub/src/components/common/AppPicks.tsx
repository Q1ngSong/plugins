import { Check } from "lucide-react";
import { APPS, AppKey } from "@/lib/api";
import { cn } from "@/lib/utils";
import { AppChip } from "./bits";

/* 选要装到哪些 app：ok 里的可以勾，done 里的显示已安装，其余不支持 */
export function AppPicks({ ok, done = [], value, onChange }: { ok: AppKey[]; done?: AppKey[]; value: AppKey[]; onChange: (v: AppKey[]) => void }) {
  return (
    <div className="grid grid-cols-2 gap-2.5">
      {APPS.map((x) => {
        const can = ok.includes(x), has = done.includes(x), on = has || value.includes(x);
        return (
          <button key={x} type="button" disabled={!can}
            onClick={() => onChange(value.includes(x) ? value.filter((v) => v !== x) : [...value, x])}
            className={cn("flex items-center gap-2.5 rounded-xl border px-3.5 py-3 text-left transition-colors",
              can && "hover:border-blue-500/50", can && on && "border-blue-500 bg-blue-500/5", !can && "cursor-not-allowed opacity-60")}>
            <span className={cn("flex h-4 w-4 flex-none items-center justify-center rounded border", on ? "border-blue-500 bg-blue-500 text-white" : "border-border")}>
              {on && <Check className="h-3 w-3" strokeWidth={3} />}
            </span>
            <AppChip app={x} />
            {!can && <span className="ml-auto text-xs text-muted-foreground">{has ? "已安装" : "不支持"}</span>}
          </button>
        );
      })}
    </div>
  );
}
