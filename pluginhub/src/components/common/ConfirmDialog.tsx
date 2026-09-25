import { AlertTriangle, Info } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";

export interface ConfirmProps {
  open: boolean;
  title: string;
  body: string;
  okText?: string;
  danger?: boolean;
  onResolve: (ok: boolean) => void;
}

/* 危险操作确认框：点遮罩不关闭，Esc 取消 */
export function ConfirmDialog({ open, title, body, okText = "确认", danger, onResolve }: ConfirmProps) {
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
          <Button variant="outline" size="sm" onClick={() => onResolve(false)}>取消</Button>
          <Button variant={danger ? "destructive" : "default"} size="sm" onClick={() => onResolve(true)}>{okText}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
