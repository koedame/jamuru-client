/**
 * DevicesTab - Audio device settings
 *
 * Design: jamuru brand (ui.pen Screens/Settings, Devices tab).
 */

import { useTranslation } from "react-i18next";
import { FormField } from "../FormField";
import { Select, SelectOption } from "../Select";
import "./TabContent.css";

export interface DeviceInfo {
  id: string;
  name: string;
  isDefault?: boolean;
}

export interface DevicesTabProps {
  /** Input devices */
  inputDevices: DeviceInfo[];
  /** Output devices */
  outputDevices: DeviceInfo[];
  /** Selected input device ID */
  selectedInputId: string | null;
  /** Selected output device ID */
  selectedOutputId: string | null;
  /** Available input channel options (e.g., Ch 1, Ch 2, etc.) */
  inputChannelOptions: SelectOption[];
  /** Available output channel options (e.g., Ch 1, Ch 2, etc.) */
  outputChannelOptions: SelectOption[];
  /** Selected input L/MONO channel */
  selectedInputChannelL: string;
  /** Selected input R channel (null or empty = mono) */
  selectedInputChannelR: string | null;
  /** Selected output L/MONO channel */
  selectedOutputChannelL: string;
  /** Selected output R channel (null or empty = mono) */
  selectedOutputChannelR: string | null;
  /** Sample rate options */
  sampleRateOptions: SelectOption[];
  /** Selected sample rate */
  selectedSampleRate: string;
  /** Buffer size options */
  bufferSizeOptions: SelectOption[];
  /** Selected buffer size */
  selectedBufferSize: string;
  /** Transmit channel options */
  transmitChannelOptions: SelectOption[];
  /** Selected transmit channels */
  selectedTransmitChannels: string;
  /** Loading state */
  isLoading?: boolean;
  /** Input device change handler */
  onInputDeviceChange: (id: string) => void;
  /** Output device change handler */
  onOutputDeviceChange: (id: string) => void;
  /** Input L channel change handler */
  onInputChannelLChange: (value: string) => void;
  /** Input R channel change handler */
  onInputChannelRChange: (value: string) => void;
  /** Output L channel change handler */
  onOutputChannelLChange: (value: string) => void;
  /** Output R channel change handler */
  onOutputChannelRChange: (value: string) => void;
  /** Sample rate change handler */
  onSampleRateChange: (value: string) => void;
  /** Buffer size change handler */
  onBufferSizeChange: (value: string) => void;
  /** Transmit channels change handler */
  onTransmitChannelsChange: (value: string) => void;
}

/** One-way latency tone class based on app-induced buffer latency (ms). */
export function latencyTone(ms: number): string {
  if (ms <= 3) return "latency-card__value--good";
  if (ms <= 6) return "latency-card__value--warn";
  return "latency-card__value--bad";
}

export function DevicesTab({
  inputDevices,
  outputDevices,
  selectedInputId,
  selectedOutputId,
  inputChannelOptions,
  outputChannelOptions,
  selectedInputChannelL,
  selectedInputChannelR,
  selectedOutputChannelL,
  selectedOutputChannelR,
  sampleRateOptions,
  selectedSampleRate,
  bufferSizeOptions,
  selectedBufferSize,
  transmitChannelOptions,
  selectedTransmitChannels,
  isLoading,
  onInputDeviceChange,
  onOutputDeviceChange,
  onInputChannelLChange,
  onInputChannelRChange,
  onOutputChannelLChange,
  onOutputChannelRChange,
  onSampleRateChange,
  onBufferSizeChange,
  onTransmitChannelsChange,
}: DevicesTabProps) {
  const { t } = useTranslation();

  const inputDeviceOptions: SelectOption[] = inputDevices.map((d) => ({
    value: d.id,
    label: d.isDefault ? `${d.name} (${t("common.default", "Default")})` : d.name,
  }));

  const outputDeviceOptions: SelectOption[] = outputDevices.map((d) => ({
    value: d.id,
    label: d.isDefault ? `${d.name} (${t("common.default", "Default")})` : d.name,
  }));

  // Add "None" option for R channel (makes it mono)
  const noneOption: SelectOption = { value: "", label: t("common.none", "None") };
  const inputRChannelOptions: SelectOption[] = [noneOption, ...inputChannelOptions];
  const outputRChannelOptions: SelectOption[] = [noneOption, ...outputChannelOptions];

  // App-induced one-way latency from the current buffer size / sample rate.
  // Displayed values are rounded first so input + output = total stays consistent.
  const bufferSamples = parseInt(selectedBufferSize, 10);
  const sampleRateHz = parseInt(selectedSampleRate, 10);
  const oneWayMs =
    bufferSamples > 0 && sampleRateHz > 0 ? (bufferSamples / sampleRateHz) * 1000 : 0;
  const oneWayDisp = Math.round(oneWayMs * 10) / 10;
  const totalDisp = Math.round(oneWayDisp * 2 * 10) / 10;

  return (
    <div className="tab-content">
      <h2 className="tab-content__title">{t("settings.devices.title", "Audio Devices")}</h2>

      <FormField
        label={t("settings.devices.inputDevice", "Input Device")}
        htmlFor="input-device"
      >
        <Select
          id="input-device"
          variant="block"
          options={inputDeviceOptions}
          value={selectedInputId ?? ""}
          onChange={onInputDeviceChange}
          disabled={isLoading}
          placeholder={t("settings.devices.selectDevice", "Select device")}
        />
      </FormField>

      <FormField
        label={t("settings.devices.outputDevice", "Output Device")}
        htmlFor="output-device"
      >
        <Select
          id="output-device"
          variant="block"
          options={outputDeviceOptions}
          value={selectedOutputId ?? ""}
          onChange={onOutputDeviceChange}
          disabled={isLoading}
          placeholder={t("settings.devices.selectDevice", "Select device")}
        />
      </FormField>

      <FormField
        label={t("settings.devices.inputChannels", "Input Channels")}
        htmlFor="input-channel-l"
      >
        <div className="channel-pair">
          <Select
            id="input-channel-l"
            variant="channel"
            prefixLabel={t("settings.devices.channelL", "L/MONO")}
            aria-label={`${t("settings.devices.inputChannels", "Input Channels")} ${t("settings.devices.channelL", "L/MONO")}`}
            options={inputChannelOptions}
            value={selectedInputChannelL}
            onChange={onInputChannelLChange}
            disabled={isLoading}
          />
          <Select
            id="input-channel-r"
            variant="channel"
            prefixLabel={t("settings.devices.channelR", "R")}
            aria-label={`${t("settings.devices.inputChannels", "Input Channels")} ${t("settings.devices.channelR", "R")}`}
            options={inputRChannelOptions}
            value={selectedInputChannelR ?? ""}
            onChange={onInputChannelRChange}
            disabled={isLoading}
          />
        </div>
      </FormField>

      <FormField
        label={t("settings.devices.outputChannels", "Output Channels")}
        htmlFor="output-channel-l"
      >
        <div className="channel-pair">
          <Select
            id="output-channel-l"
            variant="channel"
            prefixLabel={t("settings.devices.channelL", "L/MONO")}
            aria-label={`${t("settings.devices.outputChannels", "Output Channels")} ${t("settings.devices.channelL", "L/MONO")}`}
            options={outputChannelOptions}
            value={selectedOutputChannelL}
            onChange={onOutputChannelLChange}
            disabled={isLoading}
          />
          <Select
            id="output-channel-r"
            variant="channel"
            prefixLabel={t("settings.devices.channelR", "R")}
            aria-label={`${t("settings.devices.outputChannels", "Output Channels")} ${t("settings.devices.channelR", "R")}`}
            options={outputRChannelOptions}
            value={selectedOutputChannelR ?? ""}
            onChange={onOutputChannelRChange}
            disabled={isLoading}
          />
        </div>
      </FormField>

      <div className="tab-content__divider" />

      <h2 className="tab-content__title">{t("settings.devices.audioSettings", "Audio Settings")}</h2>

      <FormField
        label={t("settings.devices.transmitChannels", "Transmit Channels")}
        description={t("settings.devices.transmitChannelsDesc", "Number of audio channels to transmit")}
        orientation="row"
      >
        <div
          className="segmented"
          role="group"
          aria-label={t("settings.devices.transmitChannels", "Transmit Channels")}
        >
          {transmitChannelOptions.map((option) => {
            const active = selectedTransmitChannels === option.value;
            return (
              <button
                key={option.value}
                type="button"
                className={`segmented__option ${active ? "segmented__option--active" : ""}`}
                aria-pressed={active}
                disabled={isLoading}
                onClick={() => onTransmitChannelsChange(option.value)}
              >
                {option.label}
              </button>
            );
          })}
        </div>
      </FormField>

      <FormField
        label={t("settings.devices.sampleRate", "Sample Rate")}
        description={t("settings.devices.sampleRateDesc", "Higher improves quality and uses more bandwidth")}
        orientation="row"
        htmlFor="sample-rate"
      >
        <Select
          id="sample-rate"
          variant="inline"
          options={sampleRateOptions}
          value={selectedSampleRate}
          onChange={onSampleRateChange}
          disabled={isLoading}
        />
      </FormField>

      <FormField
        label={t("settings.devices.buffer", "Buffer Size")}
        description={t("settings.devices.bufferDesc", "Smaller = lower latency, higher CPU usage")}
        orientation="row"
        htmlFor="buffer-size"
      >
        <Select
          id="buffer-size"
          variant="inline"
          options={bufferSizeOptions}
          value={selectedBufferSize}
          onChange={onBufferSizeChange}
          disabled={isLoading}
        />
      </FormField>

      <div className="latency-card">
        <div className="latency-card__item">
          <span className="latency-card__label">{t("settings.devices.latencyInput", "Input Latency")}</span>
          <span className={`latency-card__value ${latencyTone(oneWayDisp)}`}>{oneWayDisp.toFixed(1)}ms</span>
        </div>
        <div className="latency-card__item">
          <span className="latency-card__label">{t("settings.devices.latencyOutput", "Output Latency")}</span>
          <span className={`latency-card__value ${latencyTone(oneWayDisp)}`}>{oneWayDisp.toFixed(1)}ms</span>
        </div>
        <div className="latency-card__item">
          <span className="latency-card__label">{t("settings.devices.latencyTotal", "Total Latency")}</span>
          <span className="latency-card__value latency-card__value--total">{totalDisp.toFixed(1)}ms</span>
        </div>
      </div>
    </div>
  );
}

export default DevicesTab;
