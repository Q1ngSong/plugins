import { AlertTriangle, Info } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";

/** 用户的选择：取消、确认，或者点了第三个按钮（也算同意，只是调用方知道选的是它） */
export type Answer = false | true | "alt";

export interface ConfirmProps {
  open: boolean;
  title: string;
  body: string;
  okText?: string;
  /** 第三个按钮的文字，比如「以后都直接打开」；不写就只有取消和确认 */
  altText?: string;
  danger?: boolean;
  onResolve: (answer: Answer) => void;
}

/* 确认框：点遮罩不关闭，Esc 取消 */
export function ConfirmDialog({ open, title, body, okText = "确认", altText, danger, onResolve }: ConfirmProps) {
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onResolve(false)}>
      <DialogContent className="max-w-sm" onInteractOutside={(e) => e.preventDefault()}>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {danger ? <AlertTriangle className="h-5 w-5 text-red-500" /> : <Info className="h-5 w-5 text-blue-500" />}
            {title}
          </DialogTitle>
          <DialogDescription className="max-h-[60vh] overflow-y-auto whitespace-pre-line break-words leading-relaxed">{body}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          {altText && <Button variant="outline" size="sm" className="sm:mr-auto" onClick={() => onResolve("alt")}>{altText}</Button>}
          <Button variant="outline" size="sm" onClick={() => onResolve(false)}>取消</Button>
          <Button variant={danger ? "destructive" : "default"} size="sm" onClick={() => onResolve(true)}>{okText}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
