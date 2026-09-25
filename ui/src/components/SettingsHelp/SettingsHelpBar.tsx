/**
 * SettingsHelpBar - says that help with settings is going on (ADR-043), on
 * either side, with the actions for it - above all, stopping it. Shown for
 * as long as the help lasts, so it can be stopped at any time.
 */
import "./SettingsHelp.css";

export interface SettingsHelpBarAction {
  label: string;
  onClick: () => void;
  /** Identifies the action for tests */
  testId: string;
}

export interface SettingsHelpBarProps {
  /** Who helps whom, in the user's words */
  message: string;
  /** What is happening now (waiting for an answer, the last answer), if anything */
  status?: string | null;
  actions: SettingsHelpBarAction[];
}

export function SettingsHelpBar({ message, status, actions }: SettingsHelpBarProps) {
  return (
    <div className="settings-help-bar" role="status" data-testid="settings-help-bar">
      <div className="settings-help-bar__text">
        <span className="settings-help-bar__message">{message}</span>
        {status && (
          <span className="settings-help-bar__status" data-testid="settings-help-status">
            {status}
          </span>
        )}
      </div>
      <div className="settings-help-bar__actions">
        {actions.map((action) => (
          <button
            key={action.testId}
            type="button"
            className="settings-help__button"
            data-testid={action.testId}
            onClick={action.onClick}
          >
            {action.label}
          </button>
        ))}
      </div>
    </div>
  );
}

export default SettingsHelpBar;
