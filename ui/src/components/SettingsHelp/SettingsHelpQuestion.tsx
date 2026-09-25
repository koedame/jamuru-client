/**
 * SettingsHelpQuestion - asks the helped participant to allow something
 * (ADR-043): that someone helps with their audio settings, or one change the
 * helper proposes. Nothing happens until they answer; Escape declines, and
 * clicking outside does nothing, so a stray click cannot answer for them.
 *
 * Allowing takes effect only once a question has been up for a moment: a
 * double click on Allow, or a click meant for the question before, cannot
 * approve a question that has just appeared.
 */
import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import "./SettingsHelp.css";

/** How long a new question is up before Allow takes effect */
export const ALLOW_DELAY_MS = 500;

export interface SettingsHelpQuestionProps {
  /** Whether the question is shown. Renders nothing when false. */
  open: boolean;
  /** Which question this is. A new one starts over: focus on declining, Allow held back. */
  questionKey?: string | number;
  /** What is being asked, in the user's words */
  message: string;
  allowLabel: string;
  declineLabel: string;
  onAllow: () => void;
  onDecline: () => void;
  /** Ending the whole help from the question, so it is never out of reach */
  stopLabel?: string;
  onStop?: () => void;
}

export function SettingsHelpQuestion({
  open,
  questionKey,
  message,
  allowLabel,
  declineLabel,
  onAllow,
  onDecline,
  stopLabel,
  onStop,
}: SettingsHelpQuestionProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const declineRef = useRef<HTMLButtonElement>(null);
  const [allowReady, setAllowReady] = useState(false);

  // Focus starts on declining: the safe answer when a key is pressed by accident.
  useEffect(() => {
    if (!open) return;
    setAllowReady(false);
    declineRef.current?.focus();
    const timer = setTimeout(() => setAllowReady(true), ALLOW_DELAY_MS);
    return () => clearTimeout(timer);
  }, [open, questionKey]);

  // Focus goes back where it was when the question closes.
  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => previous?.focus();
  }, [open]);

  const handleKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      onDecline();
      return;
    }
    if (e.key !== "Tab") return;
    const buttons = Array.from(dialogRef.current?.querySelectorAll("button") ?? []);
    const first = buttons[0];
    const last = buttons[buttons.length - 1];
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
        ref={dialogRef}
        className="settings-help-question"
        role="alertdialog"
        aria-modal="true"
        aria-label={message}
        data-testid="settings-help-question"
        onKeyDown={handleKeyDown}
      >
        <p className="settings-help-question__text">{message}</p>
        <div className="settings-help-question__actions">
          {stopLabel && onStop && (
            <button
              type="button"
              className="settings-help__button settings-help-question__stop"
              data-testid="settings-help-question-stop"
              onClick={onStop}
            >
              {stopLabel}
            </button>
          )}
          <button
            type="button"
            className="settings-help__button settings-help__button--primary"
            data-testid="settings-help-allow"
            aria-disabled={!allowReady}
            onClick={() => {
              if (allowReady) onAllow();
            }}
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
