/**
 * How a setting reads to the user (ADR-044 §5): the chat line after a helper
 * changed something.
 */
import type { TFunction } from "i18next";
import type { SettingChange } from "../../lib/tauri";

/** The i18n key of each setting's name, by the name a settings change carries */
const SETTING_LABEL_KEYS: Record<SettingChange["setting"], string> = {
  input_device: "settings.devices.inputDevice",
  output_device: "settings.devices.outputDevice",
  input_channel: "settings.devices.inputChannels",
  output_channel: "settings.devices.outputChannels",
  transmit_channels: "settings.devices.transmitChannels",
  buffer_size: "settings.devices.buffer",
  sample_rate: "settings.devices.sampleRate",
  preset: "settingsHelp.setting.preset",
};

/** The setting's name as the settings window shows it. */
export function settingLabel(setting: string | null | undefined, t: TFunction): string {
  const key = SETTING_LABEL_KEYS[setting as SettingChange["setting"]];
  return key ? t(key) : t("settingsHelp.setting.unknown");
}
