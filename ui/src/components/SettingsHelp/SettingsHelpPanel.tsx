/**
 * SettingsHelpPanel - the helped participant's audio settings, as the helper
 * sees and changes them (ADR-043). The same Devices tab as the settings
 * window; every choice is sent as a proposal the other person allows or not,
 * and what is shown is always what their app reports.
 */
import { DevicesTab } from "../SettingsPanel/tabs/DevicesTab";
import type { AudioSettingsTabProps } from "../SettingsPanel/useAudioSettingsTab";
import "./SettingsHelp.css";

export interface SettingsHelpPanelProps {
  /** The other person's settings as the Devices tab shows them */
  devicesTab: AudioSettingsTabProps;
  /** What is happening now (waiting for them to allow a change, the last answer) */
  status?: string | null;
  /** While a change waits for an answer, further choices wait too */
  waiting?: boolean;
}

export function SettingsHelpPanel({ devicesTab, status, waiting = false }: SettingsHelpPanelProps) {
  return (
    <div className="settings-help-panel" data-testid="settings-help-panel">
      {status && (
        <p className="settings-help-panel__status" role="status" data-testid="settings-help-panel-status">
          {status}
        </p>
      )}
      <DevicesTab {...devicesTab} isLoading={waiting} />
    </div>
  );
}

export default SettingsHelpPanel;
