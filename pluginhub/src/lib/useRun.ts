import { useCallback, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import type { Result, State } from "@/lib/api";

/* 执行一个改动类操作：同一时间只跑一个，结束后用返回的状态刷新页面，并弹出做了哪些事 */
export function useRun() {
  const qc = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const running = useRef(false);

  const run = useCallback(async (id: string, fn: () => Promise<Result>, done = "完成") => {
    if (running.current) return false;
    running.current = true;
    setBusy(id);
    try {
      const r = await fn();
      qc.setQueryData(["state"], r.state);
      const notes = r.notes ?? [];
      if (notes.length) toast.success(done, { description: notes.slice(-6).join("\n"), duration: 5000 });
      else toast.success(done);
      return true;
    } catch (e) {
      const st = (e as { state?: State }).state;
      if (st) qc.setQueryData(["state"], st);
      toast.error("操作失败", { description: (e as Error).message, duration: 8000 });
      return false;
    } finally {
      running.current = false;
      setBusy(null);
    }
  }, [qc]);

  return { busy, run };
}

export type Run = ReturnType<typeof useRun>["run"];
