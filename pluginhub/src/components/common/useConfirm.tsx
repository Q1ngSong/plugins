import { useCallback, useState } from "react";
import { ConfirmDialog, ConfirmProps } from "./ConfirmDialog";

type Ask = Omit<ConfirmProps, "open" | "onResolve">;

/* 用法：const { confirm, dialog } = useConfirm(); if (await confirm({...})) ... ; 渲染 {dialog} */
export function useConfirm() {
  const [state, setState] = useState<(Ask & { resolve: (v: boolean) => void }) | null>(null);
  const confirm = useCallback((ask: Ask) => new Promise<boolean>((resolve) => setState({ ...ask, resolve })), []);
  const dialog = state ? (
    <ConfirmDialog open title={state.title} body={state.body} okText={state.okText} danger={state.danger}
      onResolve={(v) => { state.resolve(v); setState(null); }} />
  ) : null;
  return { confirm, dialog };
}
