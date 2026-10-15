/**
 * The audio settings of the app a screen is connected to, as the settings panel
 * of a settings help shows and changes them (ADR-044 §5).
 *
 * The helper's window is connected to the helped app, so these are the helped
 * person's settings; the helped person's own screen is connected to its own app,
 * where the same panel is mirrored, so these are theirs. Either way the settings
 * arrive in no fixed order (a read, the answer to a change, an announcement), and
 * the revision says which is newest.
 */
import { useCallback, useEffect, useState } from "react";
import { useWindowEvent } from "../../hooks/useWindowEvents";
import {
  AUDIO_SETTINGS_CHANGED,
  settingsChange,
  settingsGet,
  type AudioSettings,
  type HelpRefusal,
  type SettingChange,
} from "../../lib/tauri";
import { useAudioSettingsTab } from "../SettingsPanel/useAudioSettingsTab";

const REFUSALS: readonly string[] = ["device_gone", "invalid_value", "unavailable"] satisfies HelpRefusal[];

/** The reason the other app gave for not applying a change, if it gave one we know. */
function refusalOf(error: unknown): HelpRefusal | null {
  const message = typeof error === "object" && error !== null ? (error as { message?: unknown }).message : undefined;
  return typeof message === "string" && REFUSALS.includes(message) ? (message as HelpRefusal) : null;
}

/**
 * `describeRefusal` says in words why the app did not apply a change; without
 * it a refusal is only logged.
 */
export function useAudioSettingsPanel(describeRefusal?: (reason: HelpRefusal) => string) {
  const [audio, setAudio] = useState<AudioSettings | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [changing, setChanging] = useState(false);

  const showSettings = useCallback((next: AudioSettings) => {
    setAudio((shown) => (shown && shown.revision > next.revision ? shown : next));
  }, []);
  useEffect(() => {
    settingsGet()
      .then(showSettings)
      .catch((e) => console.error("Could not read the audio settings:", e));
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
          if (reason !== null && describeRefusal) setNote(describeRefusal(reason));
        })
        .finally(() => setChanging(false));
    },
    [describeRefusal, showSettings]
  );

  return { devicesTab: useAudioSettingsTab(audio, change), note, changing };
}
