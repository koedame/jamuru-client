import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import "./LeaveDialog.css";

export interface LeaveDialogProps {
  /** Whether the dialog is visible. Renders nothing when false. */
  open: boolean;
  /** True while the leave operation is in flight ("Loading" state) - both buttons disable and the confirm button shows a
   * spinner instead of its label. */
  pending?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  title?: string;
  pendingTitle?: string;
  confirmLabel?: string;
  cancelLabel?: string;
}

/**
 * Room-leave confirmation dialog.
 * Centered, shadowed card. Escape cancels; Tab wraps focus between the two
 * buttons since focus would otherwise escape to the hidden-behind-overlay
 * content.
 */
export function LeaveDialog({
  open,
  pending = false,
  onConfirm,
  onCancel,
  title: titleProp,
  pendingTitle: pendingTitleProp,
  confirmLabel: confirmLabelProp,
  cancelLabel: cancelLabelProp,
}: LeaveDialogProps) {
  const { t } = useTranslation();
  const title = titleProp ?? t("session.leave.confirmMessage");
  const pendingTitle = pendingTitleProp ?? t("session.leave.pending");
  const confirmLabel = confirmLabelProp ?? t("session.leave.confirmButton");
  const cancelLabel = cancelLabelProp ?? t("common.button.cancel");
  const confirmRef = useRef<HTMLButtonElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (open && !pending) {
      cancelRef.current?.focus();
    }
    // Only re-focus when the dialog opens, not on every `pending` toggle.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const handleKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (pending) return;
    if (e.key === "Escape") {
      onCancel();
      return;
    }
    if (e.key !== "Tab") return;
    const first = confirmRef.current;
    const last = cancelRef.current;
    if (!first || !last) return;
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  if (!open) return null;

  return (
    <div className="leave-dialog__overlay" onClick={pending ? undefined : onCancel}>
      <div
        className="leave-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={pending ? pendingTitle : title}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={handleKeyDown}
      >
        <p className="leave-dialog__text">{pending ? pendingTitle : title}</p>
        <button
          ref={confirmRef}
          type="button"
          className="leave-dialog__confirm"
          onClick={onConfirm}
          disabled={pending}
          aria-label={pending ? pendingTitle : confirmLabel}
        >
          {pending ? <span className="leave-dialog__spinner" aria-hidden="true" /> : confirmLabel}
        </button>
        <button
          ref={cancelRef}
          type="button"
          className="leave-dialog__cancel"
          onClick={onCancel}
          disabled={pending}
        >
          {cancelLabel}
        </button>
      </div>
    </div>
  );
}
