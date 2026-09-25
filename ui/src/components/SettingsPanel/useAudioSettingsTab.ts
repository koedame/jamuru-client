/**
 * The Devices tab's props for a set of audio settings (ADR-043).
 *
 * Shared by the settings window (the app's own settings) and the panel a
 * helper uses for another participant's settings, so both offer the same
 * choices built the same way. Every choice becomes one `SettingChange`
 * handed to `change`; what is shown is whatever `audio` says, never a guess.
 */
import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import type {
  AudioDeviceInfo,
  AudioSettings,
  ChannelPair,
  ChannelSide,
  SettingChange,
} from "../../lib/tauri";
import type { DevicesTabProps } from "./tabs/DevicesTab";
import type { SelectOption } from "./Select";

/** The most channels any device is taken to have (the backend refuses higher numbers). */
const MAX_DEVICE_CHANNELS = 64;

/** Buffer sizes are labelled with their duration at 48 kHz. */
const LABEL_SAMPLE_RATE = 48000;

/**
 * How many channel choices to offer: what the device in use says it has (two
 * when it does not say), and always enough to show the pair chosen now - a
 * saved channel the device lacks stays visible instead of the picker showing
 * another number.
 */
function channelChoices(count: number | null, pair: ChannelPair | undefined): number {
  const wanted = Math.max(count ?? 2, pair?.left ?? 1, pair?.right ?? 1);
  return Math.min(wanted, MAX_DEVICE_CHANNELS);
}

/** The device shown as selected: the chosen one, or the system default when none is. */
function shownDevice(devices: AudioDeviceInfo[], chosen: string | null): string | null {
  return chosen ?? devices.find((d) => d.is_default)?.id ?? null;
}

function toDeviceInfo(device: AudioDeviceInfo) {
  return { id: device.id, name: device.name, isDefault: device.is_default };
}

export type AudioSettingsTabProps = Omit<DevicesTabProps, "isLoading">;

export function useAudioSettingsTab(
  audio: AudioSettings | null,
  change: (settingChange: SettingChange) => void
): AudioSettingsTabProps {
  const { t } = useTranslation();

  const channelOptions = (maxChannels: number): SelectOption[] =>
    Array.from({ length: maxChannels }, (_, i) => ({
      value: String(i + 1),
      label: t("settings.devices.channelOption", { channel: i + 1 }),
    }));

  // A channel picker changes one side of the pair; the app keeps the other
  // side as it is. The right picker's empty choice is mono.
  const channelChange = useCallback(
    (direction: "input" | "output", side: ChannelSide) => (value: string) => {
      const channel = value === "" ? null : parseInt(value, 10);
      if (channel !== null && isNaN(channel)) return;
      change({ setting: direction === "input" ? "input_channel" : "output_channel", side, channel });
    },
    [change]
  );

  const numberChange = (apply: (n: number) => void) => (value: string) => {
    const n = parseInt(value, 10);
    if (!isNaN(n)) apply(n);
  };

  const rightChannel = (pair: ChannelPair | undefined) =>
    pair?.right == null ? "" : String(pair.right);

  return {
    inputDevices: (audio?.input_devices ?? []).map(toDeviceInfo),
    outputDevices: (audio?.output_devices ?? []).map(toDeviceInfo),
    selectedInputId: audio ? shownDevice(audio.input_devices, audio.input_device_id) : null,
    selectedOutputId: audio ? shownDevice(audio.output_devices, audio.output_device_id) : null,
    inputChannelOptions: channelOptions(channelChoices(audio?.input_channel_count ?? null, audio?.input_channels)),
    outputChannelOptions: channelOptions(channelChoices(audio?.output_channel_count ?? null, audio?.output_channels)),
    selectedInputChannelL: String(audio?.input_channels.left ?? 1),
    selectedInputChannelR: rightChannel(audio?.input_channels),
    selectedOutputChannelL: String(audio?.output_channels.left ?? 1),
    selectedOutputChannelR: rightChannel(audio?.output_channels),
    // Option labels are built on render so they follow the UI language.
    sampleRateOptions:
      audio && audio.sample_rates.length > 0
        ? audio.sample_rates.map((sr) => ({
            value: String(sr.rate),
            label: sr.recommended ? `${sr.label} (${t("preset.recommended")})` : sr.label,
          }))
        : [{ value: "48000", label: "48 kHz" }],
    selectedSampleRate: String(audio?.sample_rate ?? 48000),
    bufferSizeOptions: (audio?.buffer_sizes ?? []).map((samples) => ({
      value: String(samples),
      label: t("settings.devices.bufferOption", {
        samples,
        ms: ((samples / LABEL_SAMPLE_RATE) * 1000).toFixed(2),
      }),
    })),
    selectedBufferSize: String(audio?.buffer_size ?? ""),
    transmitChannelOptions: [
      { value: "1", label: t("settings.devices.mono", "Mono") },
      { value: "2", label: t("settings.devices.stereo", "Stereo") },
    ],
    selectedTransmitChannels: String(audio?.transmit_channels ?? 2),
    onInputDeviceChange: (deviceId) => change({ setting: "input_device", device_id: deviceId }),
    onOutputDeviceChange: (deviceId) => change({ setting: "output_device", device_id: deviceId }),
    onInputChannelLChange: channelChange("input", "left"),
    onInputChannelRChange: channelChange("input", "right"),
    onOutputChannelLChange: channelChange("output", "left"),
    onOutputChannelRChange: channelChange("output", "right"),
    onSampleRateChange: numberChange((hz) => change({ setting: "sample_rate", hz })),
    onBufferSizeChange: numberChange((samples) => change({ setting: "buffer_size", samples })),
    onTransmitChannelsChange: numberChange((count) => {
      if (count === 1 || count === 2) change({ setting: "transmit_channels", count });
    }),
  };
}
