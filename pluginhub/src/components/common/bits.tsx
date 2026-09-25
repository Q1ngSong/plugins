import * as React from "react";
import { cn } from "@/lib/utils";
import { APP_NAME, AppKey } from "@/lib/api";

/* Claude Code 暖橙、Codex 青绿，整个界面统一用这两种颜色区分 app */
export const APP_TONE: Record<AppKey, { chip: string; text: string; bar: string; soft: string }> = {
  claude: {
    chip: "border-[#f0c4b2] bg-[#fbe9e1] text-[#c2552f] dark:border-[#5a3322] dark:bg-[#2e1a12] dark:text-[#f0936c]",
    text: "text-[#c2552f] dark:text-[#f0936c]",
    bar: "before:bg-[#c2552f] dark:before:bg-[#f0936c]",
    soft: "bg-[#fbe9e1]/60 dark:bg-[#2e1a12]/60",
  },
  codex: {
    chip: "border-[#aee0cd] bg-[#dff4ec] text-[#0b8a6a] dark:border-[#1f4b3d] dark:bg-[#0f2720] dark:text-[#4fd1a8]",
    text: "text-[#0b8a6a] dark:text-[#4fd1a8]",
    bar: "before:bg-[#0b8a6a] dark:before:bg-[#4fd1a8]",
    soft: "bg-[#dff4ec]/60 dark:bg-[#0f2720]/60",
  },
};

export function AppChip({ app, className }: { app: AppKey; className?: string }) {
  return (
    <span className={cn("inline-flex items-center gap-1.5 rounded-full border py-px pl-1.5 pr-2 text-[11px] font-semibold leading-4", APP_TONE[app].chip, className)}>
      <i className="h-1.5 w-1.5 rounded-full bg-current" />{APP_NAME[app]}
    </span>
  );
}

type Tone = "sky" | "emerald" | "amber" | "red" | "slate";
const TAG_TONE: Record<Tone, string> = {
  sky: "bg-sky-100 text-sky-700 dark:bg-sky-500/15 dark:text-sky-300",
  emerald: "bg-emerald-100 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300",
  amber: "bg-amber-100 text-amber-700 dark:bg-amber-500/15 dark:text-amber-300",
  red: "bg-red-100 text-red-600 dark:bg-red-500/15 dark:text-red-300",
  slate: "bg-slate-200 text-slate-700 dark:bg-slate-500/20 dark:text-slate-300",
};

export function Tag({ tone, mono, children }: { tone: Tone; mono?: boolean; children: React.ReactNode }) {
  return <span className={cn("inline-flex items-center gap-1 rounded-md px-1.5 text-[11px] font-semibold leading-[18px]", mono && "font-mono font-medium", TAG_TONE[tone])}>{children}</span>;
}

/* 卡片右上角的小图标按钮 */
export const IconButton = React.forwardRef<HTMLButtonElement, React.ButtonHTMLAttributes<HTMLButtonElement> & { danger?: boolean; attn?: boolean }>(
  ({ className, danger, attn, children, ...props }, ref) => (
    <button ref={ref} type="button"
      className={cn("relative inline-flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-40 [&_svg]:h-4 [&_svg]:w-4",
        danger && "hover:bg-red-50 hover:text-red-500 dark:hover:bg-red-500/10",
        attn && "text-amber-500 after:absolute after:right-1.5 after:top-1.5 after:h-1.5 after:w-1.5 after:rounded-full after:bg-amber-500",
        className)}
      {...props}>{children}</button>
  ),
);
IconButton.displayName = "IconButton";

export const Spinner = ({ className }: { className?: string }) => (
  <span className={cn("inline-block h-3.5 w-3.5 animate-spin rounded-full border-2 border-current border-r-transparent", className)} />
);
