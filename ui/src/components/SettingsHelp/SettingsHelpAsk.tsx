/**
 * SettingsHelpAsk - the two steps of asking someone for help with settings
 * (ADR-044 §5): what it means, in a few calm lines, and then who to ask.
 * Nothing is sent until the person picks someone and confirms. Escape and the
 * cancel button close it; clicking outside does nothing.
 */
import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import "./SettingsHelp.css";

export interface SettingsHelpAskCandidate {
  id: string;
  name: string;
}

export interface SettingsHelpAskProps {
  title: string;
  /** Lines said once, before choosing, each its own paragraph */
  lines: string[];
  chooseTitle: string;
  /** Said instead of the list when no one in the room can be asked */
  noneText: string;
  candidates: SettingsHelpAskCandidate[];
  chooseLabel: string;
  askLabel: string;
  cancelLabel: string;
  onAsk: (peerId: string) => void;
  onCancel: () => void;
}

export function SettingsHelpAsk({
  title,
  lines,
  chooseTitle,
  noneText,
  candidates,
  chooseLabel,
  askLabel,
  cancelLabel,
  onAsk,
  onCancel,
}: SettingsHelpAskProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const [step, setStep] = useState<"intro" | "choose">("intro");
  const [picked, setPicked] = useState<string | null>(null);
  // The one person there is to ask is already chosen.
  const chosen = picked ?? (candidates.length === 1 ? candidates[0].id : null);

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => previous?.focus();
  }, []);

  useEffect(() => {
    dialogRef.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, [step]);

  const handleKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      onCancel();
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

  return (
    <div className="settings-help-question__overlay">
      <div
        ref={dialogRef}
        className="settings-help-question"
        role="dialog"
        aria-modal="true"
        aria-label={step === "intro" ? title : chooseTitle}
        data-testid="settings-help-ask"
        onKeyDown={handleKeyDown}
      >
        {step === "intro" ? (
          <>
            <h2 className="settings-help-question__title">{title}</h2>
            {lines.map((line) => (
              <p key={line} className="settings-help-question__text">
                {line}
              </p>
            ))}
            <div className="settings-help-question__actions">
              <button
                type="button"
                className="settings-help__button settings-help__button--primary"
                data-testid="settings-help-ask-next"
                data-autofocus
                onClick={() => setStep("choose")}
              >
                {chooseLabel}
              </button>
              <button
                type="button"
                className="settings-help__button"
                data-testid="settings-help-ask-cancel"
                onClick={onCancel}
              >
                {cancelLabel}
              </button>
            </div>
          </>
        ) : (
          <>
            <h2 className="settings-help-question__title">{chooseTitle}</h2>
            {candidates.length === 0 ? (
              <p className="settings-help-question__text" data-testid="settings-help-ask-none">
                {noneText}
              </p>
            ) : (
              <div className="settings-help-ask__choices" role="radiogroup" aria-label={chooseTitle}>
                {candidates.map((candidate) => (
                  <button
                    key={candidate.id}
                    type="button"
                    role="radio"
                    aria-checked={chosen === candidate.id}
                    className="settings-help-ask__choice"
                    data-testid="settings-help-ask-choice"
                    data-peer-name={candidate.name}
                    data-autofocus={candidate.id === (chosen ?? candidates[0].id) ? "" : undefined}
                    onClick={() => setPicked(candidate.id)}
                  >
                    {candidate.name}
                  </button>
                ))}
              </div>
            )}
            <div className="settings-help-question__actions">
              <button
                type="button"
                className="settings-help__button settings-help__button--primary"
                data-testid="settings-help-ask-send"
                aria-disabled={chosen === null}
                onClick={() => {
                  if (chosen !== null) onAsk(chosen);
                }}
              >
                {askLabel}
              </button>
              <button
                type="button"
                className="settings-help__button"
                data-testid="settings-help-ask-cancel"
                onClick={onCancel}
              >
                {cancelLabel}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

export default SettingsHelpAsk;
