/**
 * SettingsHelpQuestion - asks the helped participant to allow something
 * (ADR-043): that someone helps with their audio settings, or one change the
 * helper proposes. Nothing happens until they answer; Escape declines, and
 * clicking outside does nothing, so a stray click cannot answer for them.
 */
import { useEffect, useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import "./SettingsHelp.css";

export interface SettingsHelpQuestionProps {
  /** Whether the question is shown. Renders nothing when false. */
  open: boolean;
  /** What is being asked, in the user's words */
  message: string;
  allowLabel: string;
  declineLabel: string;
  onAllow: () => void;
  onDecline: () => void;
}

export function SettingsHelpQuestion({
  open,
  message,
  allowLabel,
  declineLabel,
  onAllow,
  onDecline,
}: SettingsHelpQuestionProps) {
  const declineRef = useRef<HTMLButtonElement>(null);
  const allowRef = useRef<HTMLButtonElement>(null);

  // Focus starts on declining: the safe answer when a key is pressed by accident.
  useEffect(() => {
    if (open) declineRef.current?.focus();
  }, [open, message]);

  const handleKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      onDecline();
      return;
    }
    if (e.key !== "Tab") return;
    const first = allowRef.current;
    const last = declineRef.current;
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
    <div className="settings-help-question__overlay">
      <div
        className="settings-help-question"
        role="alertdialog"
        aria-modal="true"
        aria-label={message}
        data-testid="settings-help-question"
        onKeyDown={handleKeyDown}
      >
        <p className="settings-help-question__text">{message}</p>
        <div className="settings-help-question__actions">
          <button
            ref={allowRef}
            type="button"
            className="settings-help__button settings-help__button--primary"
            data-testid="settings-help-allow"
            onClick={onAllow}
          >
            {allowLabel}
          </button>
          <button
            ref={declineRef}
            type="button"
            className="settings-help__button"
            data-testid="settings-help-decline"
            onClick={onDecline}
          >
            {declineLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

export default SettingsHelpQuestion;
