import { useCallback, useState } from "react";
import { Answer, ConfirmDialog, ConfirmProps } from "./ConfirmDialog";

type Ask = Omit<ConfirmProps, "open" | "onResolve">;

/* 用法：const { confirm, dialog } = useConfirm(); if (await confirm({...})) ... ; 渲染 {dialog}。
   给了 altText 时，用户点第三个按钮会得到 "alt"（也是真值，照样算同意） */
export function useConfirm() {
  const [state, setState] = useState<(Ask & { resolve: (v: Answer) => void }) | null>(null);
  const confirm = useCallback((ask: Ask) => new Promise<Answer>((resolve) => setState({ ...ask, resolve })), []);
  const dialog = state ? (
    <ConfirmDialog open title={state.title} body={state.body} okText={state.okText} altText={state.altText} danger={state.danger}
      onResolve={(v) => { state.resolve(v); setState(null); }} />
  ) : null;
  return { confirm, dialog };
}
