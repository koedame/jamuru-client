/**
 * How a setting and a change to it read to the user (ADR-043): the chat line
 * after a helper changed something, and the question the helped side answers.
 */
import type { TFunction } from "i18next";
import type { AudioSettings, SettingChange } from "../../lib/tauri";

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

const PRESET_NAME_KEYS: Record<string, string> = {
  "zero-latency": "preset.zeroLatency.name",
  "ultra-low-latency": "preset.ultraLowLatency.name",
  balanced: "preset.balanced.name",
  "high-quality": "preset.highQuality.name",
};

/**
 * What `change` would set, as the helped side reads it: device names from its
 * own list, channels as the pickers name them.
 */
export function changeValue(change: SettingChange, audio: AudioSettings | null, t: TFunction): string {
  // A device the app no longer lists is not named by its id: the id can carry a serial number.
  const deviceName = (devices: AudioSettings["input_devices"] | undefined, id: string) =>
    devices?.find((d) => d.id === id)?.name ?? t("settingsHelp.value.unknownDevice");
  const channel = (n: number | null) =>
    n === null ? t("common.none", "None") : t("settings.devices.channelOption", { channel: n });
  const side = (s: "left" | "right") =>
    s === "left" ? t("settings.devices.channelL", "L/MONO") : t("settings.devices.channelR", "R");
  switch (change.setting) {
    case "input_device":
      return deviceName(audio?.input_devices, change.device_id);
    case "output_device":
      return deviceName(audio?.output_devices, change.device_id);
    case "input_channel":
    case "output_channel":
      return `${side(change.side)}: ${channel(change.channel)}`;
    case "transmit_channels":
      return change.count === 1 ? t("settings.devices.mono", "Mono") : t("settings.devices.stereo", "Stereo");
    case "buffer_size":
      return t("settingsHelp.value.samples", { samples: change.samples });
    case "sample_rate":
      return `${change.hz / 1000} kHz`;
    case "preset":
      return t(PRESET_NAME_KEYS[change.preset] ?? "settingsHelp.setting.preset");
  }
}
