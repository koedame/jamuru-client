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
import { SettingsHelpPanel } from "../components/SettingsHelp";
import { SidePanel } from "../components/SidePanel";
import { useAudioSettingsTab } from "../components/SettingsPanel/useAudioSettingsTab";
import { useWindowEvent } from "../hooks/useWindowEvents";
import {
  AUDIO_SETTINGS_CHANGED,
  helpWindowInfo,
  settingsChange,
  settingsGet,
  type AudioSettings,
  type HelpRefusal,
  type SettingChange,
} from "../lib/tauri";
import "./HelperScreen.css";

const REFUSALS: readonly string[] = ["device_gone", "invalid_value", "unavailable"] satisfies HelpRefusal[];

/** The reason the other app gave for not applying a change, if it gave one we know. */
function refusalOf(error: unknown): HelpRefusal | null {
  const message = typeof error === "object" && error !== null ? (error as { message?: unknown }).message : undefined;
  return typeof message === "string" && REFUSALS.includes(message) ? (message as HelpRefusal) : null;
}

export function HelperScreen() {
  const { t } = useTranslation();
  const [name, setName] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [audio, setAudio] = useState<AudioSettings | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [changing, setChanging] = useState(false);

  useEffect(() => {
    helpWindowInfo()
      .then((info) => setName(info.peer_name))
      .catch((e) => console.error("Could not tell who this window helps:", e));
  }, []);

  // The other app's settings arrive in no fixed order (a read, the answer to a
  // change, an announcement); the revision says which is newest.
  const showSettings = useCallback((next: AudioSettings) => {
    setAudio((shown) => (shown && shown.revision > next.revision ? shown : next));
  }, []);
  useEffect(() => {
    settingsGet()
      .then(showSettings)
      .catch((e) => console.error("Could not read the settings of the app being helped:", e));
  }, [showSettings]);
  useWindowEvent<AudioSettings>(AUDIO_SETTINGS_CHANGED, showSettings);

  const change = useCallback(
    (settingChange: SettingChange) => {
      setChanging(true);
      setNote(null);
      settingsChange(settingChange)
        .then(showSettings)
        .catch((e) => {
          console.error(`Could not change ${settingChange.setting}:`, e);
          const reason = refusalOf(e);
          if (reason !== null) setNote(t(`settingsHelp.helper.refused.${reason}`, { name: name ?? "" }));
        })
        .finally(() => setChanging(false));
    },
    [name, showSettings, t]
  );
  const devicesTab = useAudioSettingsTab(audio, change);

  if (name === null) return null;
  return (
    <div className="helper-screen" data-testid="settings-help-window">
      <p className="helper-screen__banner" role="status" data-testid="settings-help-window-banner">
        {t("settingsHelp.window.banner", { name })}
      </p>
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
    </div>
  );
}

export default HelperScreen;
