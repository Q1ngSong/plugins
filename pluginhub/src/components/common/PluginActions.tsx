import { Link2, PackagePlus, RefreshCw, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { api, canDelete, canSync, installedApps, isSkill, APP_NAME, Plugin } from "@/lib/api";
import type { Run } from "@/lib/useRun";
import type { ConfirmFn } from "@/App";
import { IconButton, Spinner } from "./bits";

/* 插件卡片和详情页右上角共用：安装到其他 app、删除（从所有 app）、同步、打开仓库 */
export function PluginActions({ p, busy, run, confirm, withDelete = true, onInstall }: {
  p: Plugin; busy: string | null; run: Run; confirm: ConfirmFn; withDelete?: boolean; onInstall?: () => void;
}) {
  const skill = isSkill(p);
  const pending = !!p.managed?.needs_update || p.apps.some((a) => a.port?.stale)
    || (!!p.skill?.ours && p.apps.some((a) => a.state && a.state !== "ok"));
  const del = async () => {
    const apps = installedApps(p).filter((a) => !a.official).map((a) => APP_NAME[a.app]).join("、") || "所有 app";
    const extra = skill
      ? p.skill?.ours ? "\n插件中心也不再管它，~/.yuwanplugins/skills 里统一存放的那份会挪进备份，不会直接删掉。" : "\n技能文件夹会挪进 ~/.pluginhub/backups，不会直接删掉。"
      : p.managed ? "\n插件中心也不再管理它，~/.yuwanplugins 里的插件文件夹会挪进备份，不会直接删掉。" : "";
    const title = skill ? "删除技能" : "删除插件";
    // 按需的没装进 app，只在技能库里
    const body = p.on_demand ? `从技能库删除「${p.name}」？${extra}` : `从 ${apps} 删除「${p.name}」？${extra}`;
    if (!(await confirm({ title, body, okText: "删除", danger: true }))) return;
    run(`del:${p.key}`, () => api.uninstall(p.key), "已删除");
  };
  const stop = (e: React.MouseEvent) => e.stopPropagation();
  return (
    <span className="flex flex-none items-center gap-0.5" onClick={stop}>
      {onInstall && p.installable.length > 0 && (
        <IconButton title={`安装到 ${p.installable.map((x) => APP_NAME[x]).join("、")}`} aria-label="安装到其他 app" disabled={!!busy} onClick={onInstall}>
          <PackagePlus />
        </IconButton>
      )}
      {withDelete && canDelete(p) && (
        <IconButton danger title="从所有 app 删除" aria-label="从所有 app 删除" disabled={!!busy} onClick={del}>
          {busy === `del:${p.key}` ? <Spinner /> : <Trash2 />}
        </IconButton>
      )}
      {canSync(p) && (
        // 锁定的插件不同步：按钮变灰点不了；外面套一层，灰掉时鼠标停上去也能看到原因
        <span title={p.managed?.locked ? "已锁定，不同步。先解锁" : undefined} className="inline-flex">
          <IconButton attn={pending && !p.managed?.locked} aria-label="同步" disabled={!!busy || !!p.managed?.locked}
            title={skill ? (pending ? "有链接要修，点击重新链接" : "重新链接并检查") : pending ? "有新版本或配置要修，点击同步" : "同步"}
            onClick={() => run(`sync:${p.key}`, () => api.sync(p.key), "已同步")}>
            {busy === `sync:${p.key}` ? <Spinner /> : <RefreshCw />}
          </IconButton>
        </span>
      )}
      {p.link && (
        <IconButton title={`打开仓库 ${p.link}`} aria-label="打开仓库"
          onClick={() => api.open(p.link).catch((e) => toast.error("打不开", { description: String(e.message ?? e) }))}>
          <Link2 />
        </IconButton>
      )}
    </span>
  );
}
