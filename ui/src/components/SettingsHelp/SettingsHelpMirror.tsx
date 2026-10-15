/**
 * SettingsHelpMirror - the helper's settings panel, on the helped person's own
 * screen (REQ-RMT-034). While the helper has the panel open the helped person
 * sees the same panel beside their own screen, so the frames that show where the
 * helper is pointing inside it have something to point at. It shows and changes
 * their own app's settings, and does not take the screen over: they can close it
 * (until the helper opens the panel again) and go on with the rest.
 */
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { SidePanel } from "../SidePanel";
import { SettingsHelpPanel } from "./SettingsHelpPanel";
import { useAudioSettingsPanel } from "./useAudioSettingsPanel";

export interface SettingsHelpMirrorProps {
  /** The helper's name */
  name: string;
  /** Whether the helper has the panel open */
  open: boolean;
}

export function SettingsHelpMirror({ name, open }: SettingsHelpMirrorProps) {
  const { t } = useTranslation();
  const { devicesTab, changing } = useAudioSettingsPanel();
  // Closed here by the helped person: it stays closed until the helper opens it again.
  const [dismissed, setDismissed] = useState(false);
  useEffect(() => {
    if (!open) setDismissed(false);
  }, [open]);

  return (
    <SidePanel
      isOpen={open && !dismissed}
      onClose={() => setDismissed(true)}
      title={t("settingsHelp.helped.panelTitle", { name })}
      modal={false}
    >
      <SettingsHelpPanel devicesTab={devicesTab} waiting={changing} />
    </SidePanel>
  );
}

export default SettingsHelpMirror;
