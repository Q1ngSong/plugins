/* 插件中心自己的更新。窗口版走 Tauri 的更新插件：读 GitHub Release 里的 latest.json，校验签名，下载装上再重启；
   浏览器版装不了自己，只问 GitHub 有没有新发布，有就带用户去下载页 */
import { useCallback, useEffect, useRef, useState } from "react";
import { api, IN_APP } from "@/lib/api";
import { isNewer, latestRelease } from "@/lib/release";

export interface Found {
  version: string;
  notes: string;
  /** 发布时间，可能没有 */
  at: string;
  /** 浏览器版用：发布页地址 */
  url?: string;
}

export interface HubUpdate {
  /** idle 还没查；checking 查询中；none 查过了没有更新；found 有新版本；downloading 下载安装中；done 装好了等重启；error 出错 */
  phase: "idle" | "checking" | "none" | "found" | "downloading" | "done" | "error";
  found: Found | null;
  /** 下载进度 0–100，不知道总大小时为 null */
  progress: number | null;
  error: string;
  /** 这个版本是不是能自己装（窗口版）；浏览器版只能打开下载页 */
  canInstall: boolean;
  check: () => Promise<void>;
  /** 窗口版：下载、安装、重启；浏览器版：打开下载页 */
  install: () => Promise<void>;
}

const EVERY = 6 * 3600_000;

/** 更新插件的报错是英文的，常见的两种换成人话 */
function explain(msg: string): string {
  if (/release JSON/i.test(msg)) return "GitHub 上还没有发布的版本，或者连不上 GitHub";
  if (/signature/i.test(msg)) return "更新包的签名对不上，没有安装";
  return msg;
}

/** 打开时查一次，之后每 6 小时查一次；查不到（断网）不打扰，只在用户手动查时报错 */
export function useHubUpdate(current: string): HubUpdate {
  const [phase, setPhase] = useState<HubUpdate["phase"]>("idle");
  const [found, setFound] = useState<Found | null>(null);
  const [progress, setProgress] = useState<number | null>(null);
  const [error, setError] = useState("");
  // 更新插件返回的对象，装的时候要用它
  const pending = useRef<import("@tauri-apps/plugin-updater").Update | null>(null);

  const check = useCallback(async (quiet = false) => {
    setPhase("checking");
    setError("");
    try {
      if (IN_APP) {
        const { check } = await import("@tauri-apps/plugin-updater");
        const u = await check();
        pending.current = u;
        if (u) setFound({ version: u.version, notes: u.body ?? "", at: u.date ?? "" });
        setPhase(u ? "found" : "none");
      } else {
        const r = await latestRelease();
        const newer = r && isNewer(r.version, current) ? r : null;
        if (newer) setFound({ version: newer.version, notes: newer.notes, at: newer.at, url: newer.url });
        setPhase(newer ? "found" : "none");
      }
    } catch (e) {
      setPhase(quiet ? "idle" : "error");
      setError(explain(String((e as Error).message ?? e)));
    }
  }, [current]);

  const install = useCallback(async () => {
    if (!found) return;
    if (!IN_APP) {
      if (found.url) await api.open(found.url);
      return;
    }
    const u = pending.current;
    if (!u) return;
    setPhase("downloading");
    setProgress(null);
    let total = 0, got = 0;
    try {
      await u.downloadAndInstall((e) => {
        if (e.event === "Started") total = e.data.contentLength ?? 0;
        if (e.event === "Progress") { got += e.data.chunkLength; if (total) setProgress(Math.min(100, Math.round((got / total) * 100))); }
      });
      setPhase("done");
      const { relaunch } = await import("@tauri-apps/plugin-process");
      await relaunch();
    } catch (e) {
      setPhase("error");
      setError(String((e as Error).message ?? e));
    }
  }, [found]);

  useEffect(() => {
    if (!current) return; // 状态还没读到，不知道自己是哪个版本，先不查
    check(true);
    const t = setInterval(() => check(true), EVERY);
    return () => clearInterval(t);
  }, [current, check]);

  return { phase, found, progress, error, canInstall: IN_APP, check: () => check(), install };
}
