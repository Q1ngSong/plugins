import { FolderGit2, Lock, PackagePlus, Save, ShieldAlert, ShieldCheck, Trash2, Wrench } from "lucide-react";
import { Button } from "@/components/ui/button";
import { AppChip, APP_TONE, IconButton, Spinner, Tag } from "@/components/common/bits";
import { api, APP_NAME, APPS, AppEntry, AppKey, howText, isSkill, Plugin, Project, short, tokens, when } from "@/lib/api";
import type { Run } from "@/lib/useRun";
import type { ConfirmFn } from "@/App";
import { cn } from "@/lib/utils";

const STATE: Record<string, [Parameters<typeof Tag>[0]["tone"], string]> = {
  ok: ["emerald", "最新"], outdated: ["amber", "有新版本"], unlinked: ["amber", "还是副本"],
  disabled: ["red", "已停用"], missing: ["red", "未安装"],
};
const HINT: Record<string, string> = {
  unlinked: "还在用复制出来的副本。点右上角的同步，改成链接到 ~/.yuwanplugins 里的插件文件夹，以后改文件或拉取就直接生效。",
  disabled: "插件被关掉了（常见原因：切换 API 服务商的工具重写了 settings.json）。同步时会自动重新打开。",
  missing: "还没装。点右上角的同步即可安装。",
  outdated: "远端有新版本，点右上角的同步拉取。",
};
/* 独立的 skill：插件中心管的那份只看链接在不在、指向对不对 */
const SKILL_STATE: Record<string, [Parameters<typeof Tag>[0]["tone"], string]> = {
  ok: ["emerald", "已链接"], unlinked: ["amber", "不是链接"], missing: ["red", "链接不见了"],
};
const SKILL_HINT: Record<string, string> = {
  unlinked: "这里放的不是指向 ~/.yuwanplugins/skills 的链接。点右上角的同步重新链接；如果是个真实的文件夹，先确认里面没有要留的东西，再从这个 app 删掉它。",
  missing: "链接不见了。点右上角的同步重新链接；开着后台检查的话，它也会自己修好。",
};

function AppCard({ p, a, busy, run, confirm }: { p: Plugin; a: AppEntry; busy: string | null; run: Run; confirm: ConfirmFn }) {
  const skill = isSkill(p);
  const ours = (!!p.managed || !!p.skill?.ours) && a.state !== undefined; // 插件中心装的那一份才有 state
  const others = p.apps.some((x) => x !== a && x.installed && x.state !== undefined);
  const del = async () => {
    const last = !ours || others ? ""
      : skill ? "\n这是最后一个装了它的 app，删掉后插件中心不再管它，~/.yuwanplugins/skills 里统一存放的那份挪进备份。"
        : "\n这是最后一个装了它的 app，删掉后插件中心不再管理它，~/.yuwanplugins 里的插件文件夹挪进备份。";
    const keep = skill && !a.linked ? "\n这里是一个真实的文件夹，会挪进 ~/.pluginhub/backups，不会直接删掉。" : "";
    const body = `从 ${APP_NAME[a.app]} 删除${skill ? "技能" : ""}「${p.name}」？只影响这一个 app。${last}${keep}`;
    if (!(await confirm({ title: `从 ${APP_NAME[a.app]} 删除`, body, okText: "删除", danger: true }))) return;
    run(`del:${a.app}:${a.id}`, () => api.uninstall(p.key, { id: a.id, app: a.app }), "已删除");
  };
  const status = ours ? (skill ? SKILL_STATE : STATE)[a.state!]
    : skill ? (a.official ? ["slate", "Codex 自带"] as const : a.linked ? ["sky", "链接"] as const : ["emerald", "已安装"] as const)
      : a.enabled === null ? ["slate", "只装在项目里"] as const : a.enabled ? ["emerald", "已启用"] as const : ["slate", "已停用"] as const;
  const hints = skill ? SKILL_HINT : HINT;
  const subject = [p.managed?.remote, p.managed?.head].find((c) => c && c.sha === a.commit)?.subject;
  const rows: [string, React.ReactNode][] = [];
  if (skill) {
    rows.push(["文件夹", <code className="break-all font-mono text-xs">{a.path || "—"}</code>]);
    if (a.linked && a.target) rows.push(["指向", <code className="break-all font-mono text-xs">{a.target}</code>]);
    if (a.version) rows.push(["版本", <code className="font-mono text-xs">{a.version}</code>]);
  } else {
    rows.push(["插件 ID", <code className="font-mono text-xs">{a.id}</code>]);
    rows.push(["版本", <code className="font-mono text-xs">{a.version || "—"}</code>]);
    if (ours) rows.push(["对应提交", a.commit ? <><code className="rounded bg-muted px-1.5 font-mono text-xs">{short(a.commit)}</code> {subject}</> : <span className="text-muted-foreground">—</span>]);
    rows.push(["开关", !a.installed ? "未安装" : a.enabled === null ? "按项目启用" : a.enabled ? "已启用" : <span className="text-red-500">已停用</span>]);
  }
  rows.push(["来源", <span className="break-all">{a.source || "—"}</span>]);
  const verifyTip = skill
    ? a.app === "codex" ? "运行 codex debug prompt-input，确认 Codex 的模型提示里有这个技能" : "核对链接和 SKILL.md。Claude Code 没有列出技能的命令，没法让它自己确认"
    : a.app === "codex" ? "运行 codex plugin list 和 codex debug prompt-input，确认 Codex 真的看得到这些 skill" : "运行 claude plugin list 和 claude plugin details，确认 Claude Code 真的认得这些 skill";

  return (
    <section className={cn("relative overflow-hidden rounded-xl border bg-card before:absolute before:inset-x-0 before:top-0 before:h-[3px]", APP_TONE[a.app].bar)}>
      <div className="flex items-center gap-2 px-4 pb-2 pt-4">
        <h3 className={cn("text-[15px] font-semibold", APP_TONE[a.app].text)}>{APP_NAME[a.app]}</h3>
        {status && <Tag tone={status[0]}>{status[1]}</Tag>}
        {a.port && <Tag tone="sky">移植自 {APP_NAME[a.port.from]}</Tag>}
        {a.port?.stale && <Tag tone="amber">源插件有新版本</Tag>}
        <span className="flex-1" />
        {a.installed && !a.official && (
          <IconButton danger title={`从 ${APP_NAME[a.app]} 删除`} aria-label={`从 ${APP_NAME[a.app]} 删除`} disabled={!!busy} onClick={del}>
            {busy === `del:${a.app}:${a.id}` ? <Spinner /> : <Trash2 />}
          </IconButton>
        )}
      </div>
      <dl className="grid grid-cols-[84px_minmax(0,1fr)] gap-x-3 gap-y-1.5 px-4 pb-3 text-[13px]">
        {rows.map(([k, v]) => [<dt key={k + "k"} className="text-muted-foreground">{k}</dt>, <dd key={k + "v"} className="min-w-0">{v}</dd>])}
        <dt className="text-muted-foreground">真实检查</dt>
        <dd className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          {a.verify ? (
            <span className={cn("inline-flex items-center gap-1", a.verify.ok ? "text-emerald-600 dark:text-emerald-400" : "text-red-500")}>
              {a.verify.ok ? <ShieldCheck className="h-3.5 w-3.5" /> : <ShieldAlert className="h-3.5 w-3.5" />}
              {a.verify.ok ? "通过" : "没通过"}<span className="text-muted-foreground">· {when(a.verify.at)} · {a.verify.detail}</span>
            </span>
          ) : <span className="text-muted-foreground">还没查过</span>}
          <button className="text-xs font-medium text-blue-500 hover:underline disabled:opacity-50" disabled={!!busy}
            title={verifyTip}
            onClick={() => run(`verify:${a.app}:${a.id}`, () => api.verify(p.key, a.app), "检查完了")}>
            {busy === `verify:${a.app}:${a.id}` ? <span className="inline-flex items-center gap-1"><Spinner />检查中…</span> : "现在检查"}
          </button>
        </dd>
      </dl>
      {ours && (hints[a.state!] || !!a.problems?.length) && (
        <div className="mx-4 mb-3 rounded-lg bg-amber-500/10 px-3 py-2 text-xs leading-relaxed text-amber-900 dark:text-amber-200">
          {!!a.problems?.length && <ul className="list-disc space-y-0.5 pl-4">{a.problems.map((x) => <li key={x}>{x}</li>)}</ul>}
          {hints[a.state!] && <p className={cn(!!a.problems?.length && "mt-1")}>
            {p.managed?.locked ? "已锁定，同步点不了：开着后台检查的话它会自动修好，也可以先解锁再点同步。" : hints[a.state!]}
          </p>}
        </div>
      )}
      {a.repair && (
        <p className={cn("mx-4 mb-3 flex items-start gap-1.5 rounded-lg px-3 py-2 text-xs leading-relaxed",
          a.repair.ok ? "bg-sky-500/10 text-sky-900 dark:text-sky-200" : "bg-red-500/10 text-red-600 dark:text-red-300")}>
          <Wrench className="mt-0.5 h-3.5 w-3.5 flex-none" />
          <span>后台检查在 {when(a.repair.at)} 发现：{a.repair.what.join("；")}。{a.repair.ok ? "已经修好了。" : "没修好，点右上角的同步再试一次。"}</span>
        </p>
      )}
      {a.port && (
        <p className="mx-4 mb-3 rounded-lg bg-sky-500/10 px-3 py-2 text-xs leading-relaxed text-sky-900 dark:text-sky-200">
          这是从 {APP_NAME[a.port.from]} 移植过来的 skill 副本，放在 <code className="font-mono">{a.port.folder}</code>，只带了 skill，钩子和 MCP 服务没有带过来。
          {a.port.stale ? ` ${APP_NAME[a.port.from]} 里的插件更新了，点右上角的同步重新复制。` : " 源插件更新后，同步时会重新复制；别在副本里改东西，会被覆盖。"}
          {a.port.source_missing && ` ${APP_NAME[a.port.from]} 里已经没有这个插件了，副本还能继续用。`}
        </p>
      )}
      <Projects list={a.projects} className={APP_TONE[a.app].soft} />
    </section>
  );
}

function Projects({ list, className }: { list: Project[]; className?: string }) {
  return <>
    <div className={cn("border-t px-4 py-2 text-xs font-semibold", className)}>用过它的项目（{list.length}）</div>
    {list.length ? (
      <ul className="divide-y">
        {list.map((x) => (
          <li key={x.path} className="flex items-center gap-3 px-4 py-2 text-[13px]">
            <FolderGit2 className="h-4 w-4 flex-none text-muted-foreground" />
            <span className="min-w-0 flex-1 break-all font-mono text-xs">{x.path}</span>
            {x.scope && <Tag tone="sky">装在这个项目里</Tag>}
            <span className="w-16 flex-none text-right text-xs text-muted-foreground">{x.count ? `${x.count} 次` : "—"}</span>
            <span className="w-20 flex-none text-right text-xs text-muted-foreground">{x.last ? when(x.last) : "—"}</span>
          </li>
        ))}
      </ul>
    ) : <div className="px-4 py-5 text-center text-xs text-muted-foreground">会话记录里还没有项目用过它</div>}
  </>;
}

/* 锁定状态，以及 ~/.yuwanplugins 里插件文件夹被改过时的提示：锁定保留修改，或者另存一份再还原 */
function LocalNotice({ p, busy, run, confirm }: { p: Plugin; busy: string | null; run: Run; confirm: ConfirmFn }) {
  const m = p.managed;
  if (!m) return null;
  const save = async () => {
    const body = `把 ~/.yuwanplugins/${m.id} 整份（包括这些修改和本地提交）另存到 ~/.pluginhub/saved，`
      + `然后还原成 ${m.branch} 分支的版本，继续同步${m.locked ? "（会顺便解锁）" : ""}。另存的文件夹会自动打开。`;
    if (await confirm({ title: "另存并还原", body, okText: "另存并还原" })) run(`save:${m.id}`, () => api.save(m.id), "已另存并还原");
  };
  if (m.modified.length) {
    return (
      <div className="mt-3 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2.5 text-xs text-amber-900 dark:text-amber-200">
        <div className="font-semibold">插件文件夹里有修改：{m.modified.join("、")}</div>
        <p className="mt-1 leading-relaxed">
          {m.locked ? "已锁定，这些修改会保留，不会被同步冲掉。"
            : "不建议在 ~/.yuwanplugins 里改插件，所以同步暂停了拉取。要保留这些修改，就锁定这个插件（不再同步）；不需要的话，另存一份再还原成仓库里的版本。"}
        </p>
        <div className="mt-2 flex flex-wrap gap-2">
          {!m.locked && (
            <Button size="sm" disabled={!!busy} onClick={() => run(`lock:${m.id}`, () => api.lock(m.id, true), "已锁定")}>
              {busy === `lock:${m.id}` ? <Spinner /> : <Lock className="h-3.5 w-3.5" />}锁定，保留修改
            </Button>
          )}
          <Button size="sm" variant="outline" disabled={!!busy} onClick={save}>
            {busy === `save:${m.id}` ? <Spinner /> : <Save className="h-3.5 w-3.5" />}另存并还原
          </Button>
        </div>
      </div>
    );
  }
  if (m.locked) {
    return (
      <p className="mt-3 flex items-center gap-1.5 rounded-lg bg-muted px-3 py-2 text-xs text-muted-foreground">
        <Lock className="h-3.5 w-3.5" />已锁定：不检查、不同步；链接或配置被改坏了，后台检查照样会修。点右上角的锁解锁。
      </p>
    );
  }
  return null;
}

/* 还没装进去的 app：写明能怎么装，或者为什么装不了 */
function MissingCard({ p, app, onInstall }: { p: Plugin; app: AppKey; onInstall: (apps: AppKey[]) => void }) {
  const o = p.install[app];
  if (!o) return null;
  return (
    <section className="flex flex-wrap items-center gap-3 rounded-xl border-[1.5px] border-dashed px-4 py-3.5">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 text-sm font-semibold">
          <span className={APP_TONE[app].text}>{APP_NAME[app]}</span><span className="text-muted-foreground">还没装</span>
        </div>
        <div className="mt-1 text-xs text-muted-foreground">{howText(o)}</div>
      </div>
      {o.how && <Button size="sm" onClick={() => onInstall([app])}><PackagePlus className="h-3.5 w-3.5" />安装到 {APP_NAME[app]}</Button>}
    </section>
  );
}

export function PluginView({ p, busy, run, confirm, onAdopt, onInstall }: {
  p: Plugin; busy: string | null; run: Run; confirm: ConfirmFn; onAdopt: (repo: string) => void; onInstall: (apps: AppKey[]) => void;
}) {
  const version = p.version || p.apps[0]?.version || "";
  // 插件中心管的 skill，链接被删了也要显示出来，好看到问题
  const cards = p.apps.filter((a) => a.installed || (isSkill(p) && a.state === "missing"));
  const shown = new Set(cards.map((a) => a.app));
  return (
    <div className="flex flex-col gap-3">
      <div className="rounded-xl border bg-card px-4 py-4">
        <div className="flex flex-wrap items-center gap-2">
          {isSkill(p) && <Tag tone="sky">技能</Tag>}
          {version && <Tag tone="slate" mono>{version}</Tag>}
          {p.apps.filter((a) => a.installed).map((a) => <AppChip key={a.app + a.id} app={a.app} />)}
          {!p.apps.some((a) => a.installed) && <span className="text-xs text-muted-foreground">哪边都没装</span>}
        </div>
        {p.description && <p className="mt-2.5 text-sm leading-relaxed text-muted-foreground">{p.description}</p>}
        {isSkill(p) && (
          <p className="mt-3 text-xs leading-relaxed text-muted-foreground">
            {p.skill?.ours ? <>统一存放在 <code className="break-all font-mono">{p.skill.folder}</code>，装了它的 app 都链接到这里。改这一份，两边都生效。</>
              : p.official ? "Codex 自带的技能，由 Codex 自己管。"
                : "独立的技能，还不归插件中心管。点右上角的安装，把它装到另一个 app：它会先挪进 ~/.yuwanplugins/skills 统一存放，两边都链接过去。"}
          </p>
        )}
        {!!p.cost && (
          <p className="mt-3 text-xs leading-relaxed text-muted-foreground">
            每次会话约占 {tokens(p.cost)} token：技能的名字和说明一直放在上下文里，模型看着说明判断什么时候用。
          </p>
        )}
        <LocalNotice p={p} busy={busy} run={run} confirm={confirm} />
        {p.managed?.error && !p.managed.modified.length && <p className="mt-3 rounded-lg bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-300">上次出错：{p.managed.error}</p>}
        {!p.managed && p.repo && !p.official && (
          <p className="mt-3 text-xs text-muted-foreground">这个插件还不归插件中心管。
            <button className="font-medium text-blue-500 hover:underline" onClick={() => onAdopt(p.repo)}>交给插件中心管理</button>
          </p>
        )}
      </div>
      {cards.map((a) => <AppCard key={a.app + a.id} p={p} a={a} busy={busy} run={run} confirm={confirm} />)}
      {APPS.filter((x) => p.install[x] && !shown.has(x)).map((x) => <MissingCard key={x} p={p} app={x} onInstall={onInstall} />)}
    </div>
  );
}
