/**
 * HelperScreen - the window someone helping works in (ADR-044 §5).
 *
 * It is the helped app's own screen, drawn by the same code (`MainScreen`) from
 * the helped app's state: this window's backend is theirs, through the relay
 * (`lib/helperBackend.ts`). What the helper does here reaches the other app, which
 * decides what it will allow; what is not allowed fails there and is not offered
 * here. The audio settings open beside it, as the other app has them.
 *
 * Closing the window stops the help.
 */
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MainScreen } from "./MainScreen";
import { SettingsHelpPanel, useAudioSettingsPanel, useHelpPresence } from "../components/SettingsHelp";
import { formatRemaining, useHelpRemaining } from "../components/SettingsHelp/useHelpRemaining";
import { SidePanel } from "../components/SidePanel";
import { helpWindowInfo, type HelpRefusal } from "../lib/tauri";
import "./HelperScreen.css";

export function HelperScreen() {
  const { t } = useTranslation();
  const [name, setName] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const { ms: remainingMs } = useHelpRemaining(true);

  useEffect(() => {
    helpWindowInfo()
      .then((info) => setName(info.peer_name))
      .catch((e) => console.error("Could not tell who this window helps:", e));
  }, []);

  // The other app's settings are read, changed and heard through the relay; what
  // it refuses to apply is said here in the helper's language.
  const describeRefusal = useCallback(
    (reason: HelpRefusal) => t(`settingsHelp.helper.refused.${reason}`, { name: name ?? "" }),
    [name, t]
  );
  const { devicesTab, note, changing } = useAudioSettingsPanel(describeRefusal);

  // Where this window's person points and focuses is shown to the helped person,
  // and where they do is shown here.
  const presence = useHelpPresence({
    role: "helper",
    enabled: name !== null,
    panelOpen: settingsOpen,
    peerName: name ?? "",
  });

  if (name === null) return null;
  return (
    <div className="helper-screen" data-testid="settings-help-window">
      <p className="helper-screen__banner" role="status" data-testid="settings-help-window-banner">
        {t("settingsHelp.window.banner", { name })}
      </p>
      <p
        className="helper-screen__presence"
        data-testid="settings-help-window-presence"
        data-delivered={presence.delivered}
      >
        <span className="helper-screen__presence-dot" aria-hidden="true" />
        {t("settingsHelp.window.presence", { name })}
      </p>
      {remainingMs !== null && (
        // Not a live region: a count that is read out every second is noise.
        <p className="helper-screen__banner" data-testid="settings-help-window-remaining">
          {t("settingsHelp.helper.remaining", { time: formatRemaining(remainingMs) })}
        </p>
      )}
      <div className="helper-screen__screen">
        <MainScreen helper={{ name }} onSettingsClick={() => setSettingsOpen(true)} />
      </div>
      <SidePanel
        isOpen={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        title={t("settingsHelp.helper.title", { name })}
      >
        <SettingsHelpPanel devicesTab={devicesTab} status={note} waiting={changing} />
      </SidePanel>
      {presence.marks}
    </div>
  );
}

export default HelperScreen;
