/**
 * SettingsHelpPanel - the helped participant's audio settings, as the helper
 * sees and changes them in their window (ADR-044 §5). The same Devices tab as the
 * settings window; every choice is applied to the other person's app, and what
 * is shown is always what their app reports.
 */
import { DevicesTab } from "../SettingsPanel/tabs/DevicesTab";
import type { AudioSettingsTabProps } from "../SettingsPanel/useAudioSettingsTab";
import "./SettingsHelp.css";

export interface SettingsHelpPanelProps {
  /** The other person's settings as the Devices tab shows them */
  devicesTab: AudioSettingsTabProps;
  /** What is worth saying about the last change (that their app could not apply it) */
  status?: string | null;
  /** While a change is on its way, further choices wait */
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
