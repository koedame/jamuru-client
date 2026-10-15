/**
 * SettingsHelpLog - what a help did, in the order it happened (ADR-044 §5):
 * the window the person being helped keeps beside the app, so the log does not
 * cover what they are working on. Whether the help goes on, how long until it
 * ends by itself, and Stop are here with the lines.
 */
import type { ReactNode } from "react";
import "./SettingsHelp.css";

export interface SettingsHelpLogLine {
  /** Identifies the line */
  id: string;
  /** When it happened, as the person reads a clock */
  time: string;
  text: string;
}

export interface SettingsHelpLogProps {
  /** Who helps, or that the help is over */
  heading: string;
  /** How long until the help ends by itself, in words; null when it is over or not known */
  remaining?: string | null;
  lines: SettingsHelpLogLine[];
  emptyText: string;
  /** Shown above the lines when there is something to tell after the help (what changed, with Undo) */
  after?: ReactNode;
  /** Present while the help goes on */
  stopLabel?: string;
  onStop?: () => void;
}

export function SettingsHelpLog({
  heading,
  remaining,
  lines,
  emptyText,
  after,
  stopLabel,
  onStop,
}: SettingsHelpLogProps) {
  return (
    <div className="settings-help-log" data-testid="settings-help-log">
      <div className="settings-help-log__head">
        <h1 className="settings-help-log__heading" data-testid="settings-help-log-heading">
          {heading}
        </h1>
        {remaining && (
          <p className="settings-help-log__remaining" data-testid="settings-help-log-remaining">
            {remaining}
          </p>
        )}
        {stopLabel && onStop && (
          <button type="button" className="settings-help__button" data-testid="settings-help-log-stop" onClick={onStop}>
            {stopLabel}
          </button>
        )}
      </div>
      {after}
      {lines.length === 0 ? (
        <p className="settings-help-log__empty">{emptyText}</p>
      ) : (
        <ol className="settings-help-log__lines" data-testid="settings-help-log-lines">
          {lines.map((line) => (
            <li key={line.id} className="settings-help-log__line">
              <time className="settings-help-log__time">{line.time}</time>
              <span>{line.text}</span>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

export default SettingsHelpLog;
