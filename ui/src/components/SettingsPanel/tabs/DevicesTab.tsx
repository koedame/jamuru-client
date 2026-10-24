/**
 * DevicesTab - Audio device settings
 *
 * Design: jamuru brand (ui.pen Screens/Settings, Devices tab).
 */

import { useTranslation } from "react-i18next";
import type { WasapiMode, WasapiModeAdvice, WasapiSettings } from "../../../lib/tauri";
import { FormField } from "../FormField";
import { Select, SelectOption } from "../Select";
import "./TabContent.css";

export interface DeviceInfo {
  id: string;
  name: string;
  isDefault?: boolean;
  /** A Bluetooth device: not suited to the lowest latency and best quality */
  isBluetooth?: boolean;
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
  /**
   * How a WASAPI device is opened, with what each way would do for the
   * devices in use (Windows). The choice is left out when absent.
   */
  wasapi?: WasapiSettings | null;
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
  /** WASAPI mode change handler */
  onWasapiModeChange?: (mode: WasapiMode) => void;
}

/** One-way latency tone class based on app-induced buffer latency (ms). */
export function latencyTone(ms: number): string {
  if (ms <= 3) return "latency-card__value--good";
  if (ms <= 6) return "latency-card__value--warn";
  return "latency-card__value--bad";
}

/** Microseconds as milliseconds to one decimal place */
function usToMs(us: number): number {
  return Math.round(us / 100) / 10;
}

/**
 * The way the devices in use run at: the one chosen, or the standard way
 * when the one chosen does not open them (the app falls back to it).
 */
export function wasapiModeInEffect(wasapi: WasapiSettings): WasapiModeAdvice | undefined {
  const chosen = wasapi.advice.modes.find((advice) => advice.mode === wasapi.mode);
  if (chosen && chosen.verdict !== "unavailable") return chosen;
  return wasapi.advice.modes.find((advice) => advice.mode === "shared");
}

/**
 * One-way latency a side adds, in ms: the buffer, or the WASAPI period the
 * device runs at when that is longer (a device delivers a period at a time).
 */
export function oneWayLatencyMs(bufferMs: number, periodUs: number | null | undefined): number {
  return periodUs == null ? bufferMs : Math.max(bufferMs, periodUs / 1000);
}

interface WasapiModeFieldProps {
  wasapi: WasapiSettings;
  disabled?: boolean;
  onChange: (mode: WasapiMode) => void;
}

/** The choice of how a WASAPI device is opened, each way marked for the devices in use */
function WasapiModeField({ wasapi, disabled, onChange }: WasapiModeFieldProps) {
  const { t } = useTranslation();
  const label = t("settings.devices.wasapi.label");

  if (!wasapi.advice.applies) {
    return (
      <div className="form-field form-field--row">
        <div className="form-field__label-group">
          <span className="form-field__label">{label}</span>
          <p className="form-field__description">{t("settings.devices.wasapi.notApplied")}</p>
        </div>
      </div>
    );
  }

  const periodText = (advice: WasapiModeAdvice): string | null => {
    const input = advice.input_period_us;
    const output = advice.output_period_us;
    if (input != null && output != null && usToMs(input) !== usToMs(output)) {
      return t("settings.devices.wasapi.periodBoth", { input: usToMs(input).toFixed(1), output: usToMs(output).toFixed(1) });
    }
    const either = input ?? output;
    return either == null ? null : t("settings.devices.wasapi.period", { ms: usToMs(either).toFixed(1) });
  };

  return (
    <fieldset className="wasapi-mode" disabled={disabled}>
      <legend className="wasapi-mode__legend">{label}</legend>
      <p className="form-field__description">{t("settings.devices.wasapi.description")}</p>
      <div className="wasapi-mode__options">
        {wasapi.advice.modes.map((advice) => {
          const checked = wasapi.mode === advice.mode;
          const unavailable = advice.verdict === "unavailable";
          const period = unavailable ? null : periodText(advice);
          const id = `wasapi-mode-${advice.mode}`;
          return (
            <label
              key={advice.mode}
              htmlFor={id}
              className={[
                "wasapi-mode__option",
                checked ? "wasapi-mode__option--checked" : "",
                unavailable ? "wasapi-mode__option--unavailable" : "",
              ].join(" ")}
            >
              <input
                id={id}
                type="radio"
                name="wasapi-mode"
                className="wasapi-mode__radio"
                value={advice.mode}
                checked={checked}
                disabled={unavailable}
                aria-describedby={`${id}-note`}
                onChange={() => onChange(advice.mode)}
              />
              <span className="wasapi-mode__body">
                <span className="wasapi-mode__head">
                  <span className="wasapi-mode__title">
                    <span className="wasapi-mode__name">{t(`settings.devices.wasapi.modes.${advice.mode}`)}</span>
                    <span className={`wasapi-mode__verdict wasapi-mode__verdict--${advice.verdict}`}>
                      {t(`settings.devices.wasapi.verdict.${advice.verdict}`)}
                    </span>
                  </span>
                  {period && <span className="wasapi-mode__period">{period}</span>}
                </span>
                <span id={`${id}-note`} className="wasapi-mode__notes">
                  <span className="wasapi-mode__reason">{t(`settings.devices.wasapi.reason.${advice.reason}`)}</span>
                  {advice.mode === "exclusive" && !unavailable && (
                    <span className="wasapi-mode__caution">{t("settings.devices.wasapi.exclusiveCaution")}</span>
                  )}
                </span>
              </span>
            </label>
          );
        })}
      </div>
    </fieldset>
  );
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
  wasapi,
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
  onWasapiModeChange = () => {},
}: DevicesTabProps) {
  const { t } = useTranslation();

  const deviceOption = (d: DeviceInfo): SelectOption => {
    const marks = [
      d.isDefault ? t("common.default", "Default") : null,
      d.isBluetooth ? t("settings.devices.bluetoothMark", "Bluetooth") : null,
    ].filter(Boolean);
    return { value: d.id, label: marks.length > 0 ? `${d.name} (${marks.join(", ")})` : d.name };
  };
  const inputDeviceOptions: SelectOption[] = inputDevices.map(deviceOption);
  const outputDeviceOptions: SelectOption[] = outputDevices.map(deviceOption);

  // A Bluetooth device is warned about, never refused.
  const bluetoothWarning = (devices: DeviceInfo[], selectedId: string | null) =>
    devices.find((d) => d.id === selectedId)?.isBluetooth
      ? t(
          "settings.devices.bluetoothWarning",
          "Bluetooth devices add delay of their own, and their sound quality drops while the microphone is in use. For the lowest latency and the best sound, use wired headphones or an audio interface."
        )
      : undefined;

  // Add "None" option for R channel (makes it mono)
  const noneOption: SelectOption = { value: "", label: t("common.none", "None") };
  const inputRChannelOptions: SelectOption[] = [noneOption, ...inputChannelOptions];
  const outputRChannelOptions: SelectOption[] = [noneOption, ...outputChannelOptions];

  // App-induced one-way latency from the current buffer size / sample rate,
  // or the WASAPI period a device runs at when that is longer.
  // Displayed values are rounded first so input + output = total stays consistent.
  const bufferSamples = parseInt(selectedBufferSize, 10);
  const sampleRateHz = parseInt(selectedSampleRate, 10);
  const bufferMs =
    bufferSamples > 0 && sampleRateHz > 0 ? (bufferSamples / sampleRateHz) * 1000 : 0;
  const inEffect = wasapi?.advice.applies ? wasapiModeInEffect(wasapi) : undefined;
  const round = (ms: number) => Math.round(ms * 10) / 10;
  const inputDisp = round(oneWayLatencyMs(bufferMs, inEffect?.input_period_us));
  const outputDisp = round(oneWayLatencyMs(bufferMs, inEffect?.output_period_us));
  const totalDisp = round(inputDisp + outputDisp);

  return (
    <div className="tab-content">
      <h2 className="tab-content__title">{t("settings.devices.title", "Audio Devices")}</h2>

      <FormField
        label={t("settings.devices.inputDevice", "Input Device")}
        htmlFor="input-device"
        warning={bluetoothWarning(inputDevices, selectedInputId)}
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
        warning={bluetoothWarning(outputDevices, selectedOutputId)}
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
        description={t("settings.devices.sampleRateDesc", "Higher gives better quality but uses more bandwidth")}
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

      {wasapi && <WasapiModeField wasapi={wasapi} disabled={isLoading} onChange={onWasapiModeChange} />}

      <div className="latency-card">
        <div className="latency-card__item">
          <span className="latency-card__label">{t("settings.devices.latencyInput", "Input Latency")}</span>
          <span className={`latency-card__value ${latencyTone(inputDisp)}`}>{inputDisp.toFixed(1)}ms</span>
        </div>
        <div className="latency-card__item">
          <span className="latency-card__label">{t("settings.devices.latencyOutput", "Output Latency")}</span>
          <span className={`latency-card__value ${latencyTone(outputDisp)}`}>{outputDisp.toFixed(1)}ms</span>
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
