import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent as ReactKeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { Input } from "../SettingsPanel/Input";
import "./EntryNameDialog.css";

/** The longest display name the backend accepts. */
export const MAX_NAME_LENGTH = 32;

export interface EntryNameDialogProps {
  /** Whether the dialog is visible. Renders nothing when false. */
  open: boolean;
  /** The name in the field when the dialog opens: the one used last time, or the guest name. */
  initialName: string;
  /** Label of the button that goes on to create or enter the room. */
  confirmLabel: string;
  /** True while the name is being saved - the fields and buttons disable. */
  pending?: boolean;
  /** Why the name could not be used, shown under the field. */
  error?: string;
  /** The person chose a name. `remember` is whether to stop asking from now on. */
  onConfirm: (name: string, remember: boolean) => void;
  onCancel: () => void;
}

/**
 * Asks which name to enter a room with, before a room is created or joined.
 * The field starts with `initialName`; an empty name cannot be confirmed.
 * Enter confirms, Escape cancels, and Tab stays inside the dialog.
 */
export function EntryNameDialog({ open, ...props }: EntryNameDialogProps) {
  // Mounted only while open, so each opening starts from `initialName`, unticked.
  return open ? <OpenEntryNameDialog {...props} /> : null;
}

function OpenEntryNameDialog({
  initialName,
  confirmLabel,
  pending = false,
  error,
  onConfirm,
  onCancel,
}: Omit<EntryNameDialogProps, "open">) {
  const { t } = useTranslation();
  const [name, setName] = useState(initialName);
  const [remember, setRemember] = useState(false);
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const errorId = useId();

  useEffect(() => {
    const field = dialogRef.current?.querySelector<HTMLInputElement>('input[type="text"]');
    field?.focus();
    field?.select();
  }, []);

  const trimmed = name.trim();
  const canConfirm = trimmed.length > 0 && !pending;

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (canConfirm) onConfirm(trimmed, remember);
  };

  const handleKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape" && !pending) {
      onCancel();
      return;
    }
    if (e.key !== "Tab") return;
    const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(
      "input:not(:disabled), button:not(:disabled)"
    );
    if (!focusable || focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="entry-name-dialog__overlay" onClick={pending ? undefined : onCancel}>
      <div
        ref={dialogRef}
        className="entry-name-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={handleKeyDown}
      >
        <form className="entry-name-dialog__form" onSubmit={handleSubmit}>
          <h2 id={titleId} className="entry-name-dialog__title">
            {t("session.entryName.title")}
          </h2>
          <label className="entry-name-dialog__field">
            <span className="entry-name-dialog__label">{t("session.entryName.label")}</span>
            <Input
              type="text"
              value={name}
              onChange={setName}
              maxLength={MAX_NAME_LENGTH}
              disabled={pending}
              autoComplete="off"
              hasError={!!error}
              aria-describedby={error ? errorId : undefined}
            />
          </label>
          <p className="entry-name-dialog__hint">{t("session.entryName.hint")}</p>
          {error && (
            <p id={errorId} className="entry-name-dialog__error" role="alert">
              {error}
            </p>
          )}
          <label className="entry-name-dialog__remember">
            <input
              type="checkbox"
              checked={remember}
              onChange={(e) => setRemember(e.target.checked)}
              disabled={pending}
            />
            <span>{t("session.entryName.remember")}</span>
          </label>
          <div className="entry-name-dialog__actions">
            <button type="submit" className="entry-name-dialog__confirm" disabled={!canConfirm}>
              {confirmLabel}
            </button>
            <button
              type="button"
              className="entry-name-dialog__cancel"
              onClick={onCancel}
              disabled={pending}
            >
              {t("common.button.cancel")}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
