/**
 * SettingsPanelAdapter - Wraps Storybook SettingsPanel with Tauri API
 * Connects Props-based SettingsPanel to Tauri backend
 */
import { useState, useEffect, useCallback, useRef } from "react";
import { useTranslation } from "react-i18next";
import {
  AudioDeviceInfo,
  AUDIO_SETTINGS_CHANGED,
  settingsGet,
  settingsChange,
  configLoad,
  configSetUsageReporting,
  configGetPeerName,
  configSetPeerName,
  configGetServerUrl,
  configSetServerUrl,
  configGetEffectiveServerUrl,
  configSetLanguage,
  diagnosticsRunComplete,
  type AudioPresetId,
  type AudioSettings,
  type ChannelPair,
  type CompleteDiagnosticsResult,
  type RecommendedPreset,
  type SettingChange,
  type ChannelSide,
  logOpenDir,
  usagePreview as readUsagePreview,
} from "../../lib/tauri";
import { useWindowEvent } from "../../hooks/useWindowEvents";
import { SettingsPanel, type SettingsTabId, type SelectOption, type DeviceInfo, type Language } from "./index";

export interface SettingsPanelAdapterProps {
  /** Initial tab to show */
  initialTab?: SettingsTabId;
  /** Callback when settings change */
  onSettingsChange?: () => void;
}

// Convert Tauri device format to SettingsPanel format
function toDeviceInfo(device: AudioDeviceInfo): DeviceInfo {
  return {
    id: device.id,
    name: device.name,
    isDefault: device.is_default,
  };
}

/**
 * How many channel choices to offer: what the device in use says it has, or
 * - when it does not say - enough to show the pair chosen now (at least two).
 */
function channelChoices(count: number | null, pair: ChannelPair | undefined): number {
  return count ?? Math.max(2, pair?.left ?? 1, pair?.right ?? 1);
}

/** The device shown as selected: the chosen one, or the system default when none is. */
function shownDevice(devices: AudioDeviceInfo[], chosen: string | null): string | null {
  return chosen ?? devices.find((d) => d.is_default)?.id ?? null;
}

/** The preset each diagnostics recommendation names. */
const PRESET_OF_RECOMMENDATION: Record<RecommendedPreset, AudioPresetId> = {
  ZeroLatency: "zero-latency",
  UltraLowLatency: "ultra-low-latency",
  Balanced: "balanced",
  HighQuality: "high-quality",
};

/** Buffer sizes are labelled with their duration at 48 kHz. */
const LABEL_SAMPLE_RATE = 48000;

export function SettingsPanelAdapter({
  initialTab = "general",
  onSettingsChange,
}: SettingsPanelAdapterProps) {
  const { i18n, t } = useTranslation();

  // State
  // Audio settings as the backend has them in effect (ADR-043). Replaced
  // wholesale after each change - including one made from outside this
  // window - rather than patched field by field.
  const [audio, setAudio] = useState<AudioSettings | null>(null);
  // Settings arrive from three places (loading, the answer to a change, the
  // announcement of any change) in no fixed order; the revision decides which
  // is newest, so an older one never replaces a newer one.
  const showSettings = useCallback((next: AudioSettings) => {
    setAudio((shown) => (shown && shown.revision > next.revision ? shown : next));
  }, []);
  const [displayName, setDisplayName] = useState<string>("User");
  const [displayNameError, setDisplayNameError] = useState<string | undefined>();
  const [serverUrl, setServerUrl] = useState<string>("");
  const [effectiveServerUrl, setEffectiveServerUrl] = useState<string>("");
  const [isLoading, setIsLoading] = useState(true);
  const [language, setLanguage] = useState<Language>(
    (i18n.language?.startsWith("ja") ? "ja" : "en") as Language
  );

  // Keeps the select in sync when the language changes for a reason other
  // than this panel's own handler - e.g. the startup sync in App.tsx applying
  // the saved config after this component already mounted with a stale
  // (localStorage-detected) value.
  useEffect(() => {
    setLanguage((i18n.language?.startsWith("ja") ? "ja" : "en") as Language);
  }, [i18n.language]);

  // Diagnostics state
  const [diagnosticsState, setDiagnosticsState] = useState<"idle" | "running" | "complete">("idle");
  const [diagnosticsProgress, setDiagnosticsProgress] = useState<number>(0);
  const [diagnosticsProgressMessage, setDiagnosticsProgressMessage] = useState<string>("");
  const [diagnosticsResult, setDiagnosticsResult] = useState<CompleteDiagnosticsResult | undefined>(undefined);
  // Diagnostics can't be truly aborted mid-run; this lets Cancel drop the
  // pending result and stop the simulated progress so the UI returns to idle.
  const diagnosticsCancelledRef = useRef(false);
  const diagnosticsIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const [logFolder, setLogFolder] = useState<string | null>(null);
  const [logFolderError, setLogFolderError] = useState<string | null>(null);

  // Usage reporting (off unless the user turns it on)
  const [usageReporting, setUsageReporting] = useState(false);
  const [usagePreview, setUsagePreview] = useState<string | null>(null);
  const [usagePreviewError, setUsagePreviewError] = useState<string | null>(null);

  // If the panel unmounts mid-run (e.g. the settings window closes while
  // diagnostics are running), stop the progress-simulation interval and
  // mark the run as cancelled so the pending diagnosticsRunComplete() result
  // is dropped instead of calling setState on an unmounted component.
  useEffect(() => {
    return () => {
      diagnosticsCancelledRef.current = true;
      if (diagnosticsIntervalRef.current) clearInterval(diagnosticsIntervalRef.current);
    };
  }, []);

  // Option labels are built on render so they follow the UI language.
  const sampleRateOptions: SelectOption[] =
    audio && audio.sample_rates.length > 0
      ? audio.sample_rates.map((sr) => ({
          value: String(sr.rate),
          label: sr.recommended ? `${sr.label} (${t("preset.recommended")})` : sr.label,
        }))
      : [{ value: "48000", label: "48 kHz" }];

  const bufferSizeOptions: SelectOption[] = (audio?.buffer_sizes ?? []).map((samples) => ({
    value: String(samples),
    label: t("settings.devices.bufferOption", {
      samples,
      ms: ((samples / LABEL_SAMPLE_RATE) * 1000).toFixed(2),
    }),
  }));

  // Transmit channel options
  const transmitChannelOptions: SelectOption[] = [
    { value: "1", label: t("settings.devices.mono", "Mono") },
    { value: "2", label: t("settings.devices.stereo", "Stereo") },
  ];

  const buildChannelOptions = (maxChannels: number): SelectOption[] =>
    Array.from({ length: maxChannels }, (_, i) => ({
      value: String(i + 1),
      label: t("settings.devices.channelOption", { channel: i + 1 }),
    }));

  // Load settings on mount
  useEffect(() => {
    const loadSettings = async () => {
      try {
        setIsLoading(true);

        const [current, savedPeerName, savedServerUrl, currentEffectiveServerUrl] = await Promise.all([
          settingsGet(),
          configGetPeerName().catch(() => "User"),
          configGetServerUrl().catch(() => null),
          configGetEffectiveServerUrl().catch(() => ""),
        ]);

        showSettings(current);
        setDisplayName(savedPeerName);
        setServerUrl(savedServerUrl ?? "");
        setEffectiveServerUrl(currentEffectiveServerUrl);
      } catch (err) {
        console.error("Failed to load settings:", err);
      } finally {
        setIsLoading(false);
      }
    };

    loadSettings();
  }, []);

  // A change made anywhere - another window, the E2E control channel, a peer
  // helping with the settings - arrives here with the settings now in effect.
  useWindowEvent<AudioSettings>(AUDIO_SETTINGS_CHANGED, showSettings);

  // Applies one change and shows what is now in effect.
  const change = useCallback(
    async (settingChange: SettingChange) => {
      try {
        showSettings(await settingsChange(settingChange));
        onSettingsChange?.();
      } catch (err) {
        console.error(`Failed to change ${settingChange.setting}:`, err);
      }
    },
    [onSettingsChange, showSettings]
  );

  useEffect(() => {
    configLoad()
      .then((config) => setUsageReporting(config?.usage_reporting === true))
      .catch((err) => console.error("Failed to read the usage reporting setting:", err));
  }, []);

  // Handlers
  const handleLanguageChange = useCallback(
    (newLanguage: Language) => {
      setLanguage(newLanguage);
      i18n.changeLanguage(newLanguage);
      // Persists the choice and notifies every other open window.
      configSetLanguage(newLanguage).catch((err) => {
        console.error("Failed to save the language:", err);
      });
    },
    [i18n]
  );

  const handleServerUrlChange = useCallback(async (newUrl: string) => {
    const trimmed = newUrl.trim();
    setServerUrl(trimmed);
    try {
      await configSetServerUrl(trimmed || null);
      setEffectiveServerUrl(await configGetEffectiveServerUrl());
    } catch (err) {
      console.error("Failed to set the signaling server URL:", err);
    }
  }, []);

  const handleDisplayNameChange = useCallback(
    async (newName: string) => {
      setDisplayName(newName);
      setDisplayNameError(undefined);

      const trimmed = newName.trim();
      if (trimmed.length === 0) {
        setDisplayNameError(t("settings.profile.nameRequired"));
        return;
      }
      if (trimmed.length > 32) {
        setDisplayNameError(t("settings.profile.nameTooLong"));
        return;
      }

      try {
        await configSetPeerName(trimmed);
        onSettingsChange?.();
      } catch (err) {
        setDisplayNameError(err instanceof Error ? err.message : String(err));
      }
    },
    [onSettingsChange, t]
  );

  const handleInputDeviceChange = useCallback(
    (deviceId: string) => change({ setting: "input_device", device_id: deviceId }),
    [change]
  );

  const handleOutputDeviceChange = useCallback(
    (deviceId: string) => change({ setting: "output_device", device_id: deviceId }),
    [change]
  );

  const handleBufferSizeChange = useCallback(
    (value: string) => {
      const samples = parseInt(value, 10);
      if (!isNaN(samples)) change({ setting: "buffer_size", samples });
    },
    [change]
  );

  const handleSampleRateChange = useCallback(
    (value: string) => {
      const hz = parseInt(value, 10);
      if (!isNaN(hz)) change({ setting: "sample_rate", hz });
    },
    [change]
  );

  // A channel picker changes one side of the pair; the app keeps the other
  // side as it is. The right picker's empty choice is mono.
  const handleChannelChange = useCallback(
    (direction: "input" | "output", side: ChannelSide, value: string) => {
      const channel = value === "" ? null : parseInt(value, 10);
      if (channel !== null && isNaN(channel)) return;
      change({ setting: direction === "input" ? "input_channel" : "output_channel", side, channel });
    },
    [change]
  );

  const handleTransmitChannelsChange = useCallback(
    (value: string) => {
      const count = parseInt(value, 10);
      if (count === 1 || count === 2) change({ setting: "transmit_channels", count });
    },
    [change]
  );

  // Run diagnostics handler
  const handleRunDiagnostics = useCallback(async () => {
    // Reset and start
    diagnosticsCancelledRef.current = false;
    setDiagnosticsState("running");
    setDiagnosticsProgress(0);
    setDiagnosticsProgressMessage(t("settings.diagnostics.stepNetwork", "Checking network connection"));
    setDiagnosticsResult(undefined);

    try {
      // Simulate progress updates (Tauri API doesn't provide progress callbacks)
      diagnosticsIntervalRef.current = setInterval(() => {
        setDiagnosticsProgress((prev) => {
          if (prev < 90) {
            const newProgress = prev + Math.random() * 15;
            if (newProgress < 33) {
              setDiagnosticsProgressMessage(t("settings.diagnostics.stepNetwork", "Checking network connection"));
            } else if (newProgress < 66) {
              setDiagnosticsProgressMessage(t("settings.diagnostics.stepAudio", "Checking audio devices"));
            } else {
              setDiagnosticsProgressMessage(t("settings.diagnostics.stepCpu", "Testing CPU performance"));
            }
            return Math.min(newProgress, 90);
          }
          return prev;
        });
      }, 200);

      // Run the actual diagnostics
      const result = await diagnosticsRunComplete();

      // Stop progress simulation
      if (diagnosticsIntervalRef.current) clearInterval(diagnosticsIntervalRef.current);

      // Ignore the result if the run was cancelled.
      if (diagnosticsCancelledRef.current) return;

      // Complete
      setDiagnosticsProgress(100);
      setDiagnosticsResult(result);
      setDiagnosticsState("complete");
    } catch (err) {
      if (diagnosticsIntervalRef.current) clearInterval(diagnosticsIntervalRef.current);
      if (diagnosticsCancelledRef.current) return;
      console.error("Diagnostics failed:", err);
      // On error, return to idle state
      setDiagnosticsState("idle");
      setDiagnosticsProgress(0);
      setDiagnosticsProgressMessage("");
    }
  }, [t]);

  const handleOpenLogFolder = useCallback(async () => {
    try {
      setLogFolder(await logOpenDir());
      setLogFolderError(null);
    } catch (err) {
      console.error("Could not open the log folder:", err);
      setLogFolderError(String(err));
    }
  }, []);

  const handleShowUsagePreview = useCallback(async () => {
    try {
      setUsagePreview(await readUsagePreview());
      setUsagePreviewError(null);
    } catch (err) {
      console.error("Could not read what would be sent:", err);
      setUsagePreviewError(String(err));
    }
  }, []);

  const handleUsageReportingChange = useCallback(
    async (enabled: boolean) => {
      setUsageReporting(enabled);
      try {
        await configSetUsageReporting(enabled);
        setUsagePreviewError(null);
        // What is shown follows the setting: turning it off empties it.
        if (usagePreview !== null) await handleShowUsagePreview();
      } catch (err) {
        console.error("Failed to save the usage reporting setting:", err);
        setUsageReporting(!enabled);
        setUsagePreviewError(String(err));
      }
    },
    [usagePreview, handleShowUsagePreview]
  );

  // Cancel diagnostics: drop the pending result and return to idle.
  const handleCancelDiagnostics = useCallback(() => {
    diagnosticsCancelledRef.current = true;
    if (diagnosticsIntervalRef.current) clearInterval(diagnosticsIntervalRef.current);
    setDiagnosticsState("idle");
    setDiagnosticsProgress(0);
    setDiagnosticsProgressMessage("");
  }, []);

  // Apply recommended preset handler
  const handleApplyPreset = useCallback(
    (preset: RecommendedPreset) => change({ setting: "preset", preset: PRESET_OF_RECOMMENDATION[preset] }),
    [change]
  );

  return (
    <SettingsPanel
      initialTab={initialTab}
      language={language}
      serverUrl={serverUrl}
      effectiveServerUrl={effectiveServerUrl}
      displayName={displayName}
      displayNameError={displayNameError}
      inputDevices={(audio?.input_devices ?? []).map(toDeviceInfo)}
      outputDevices={(audio?.output_devices ?? []).map(toDeviceInfo)}
      selectedInputId={audio ? shownDevice(audio.input_devices, audio.input_device_id) : null}
      selectedOutputId={audio ? shownDevice(audio.output_devices, audio.output_device_id) : null}
      inputChannelOptions={buildChannelOptions(
        channelChoices(audio?.input_channel_count ?? null, audio?.input_channels)
      )}
      outputChannelOptions={buildChannelOptions(
        channelChoices(audio?.output_channel_count ?? null, audio?.output_channels)
      )}
      selectedInputChannelL={String(audio?.input_channels.left ?? 1)}
      selectedInputChannelR={audio?.input_channels.right == null ? "" : String(audio.input_channels.right)}
      selectedOutputChannelL={String(audio?.output_channels.left ?? 1)}
      selectedOutputChannelR={audio?.output_channels.right == null ? "" : String(audio.output_channels.right)}
      sampleRateOptions={sampleRateOptions}
      selectedSampleRate={String(audio?.sample_rate ?? 48000)}
      bufferSizeOptions={bufferSizeOptions}
      selectedBufferSize={String(audio?.buffer_size ?? "")}
      transmitChannelOptions={transmitChannelOptions}
      selectedTransmitChannels={String(audio?.transmit_channels ?? 2)}
      isLoading={isLoading}
      onLanguageChange={handleLanguageChange}
      onServerUrlChange={handleServerUrlChange}
      onDisplayNameChange={handleDisplayNameChange}
      onInputDeviceChange={handleInputDeviceChange}
      onOutputDeviceChange={handleOutputDeviceChange}
      onInputChannelLChange={(value) => handleChannelChange("input", "left", value)}
      onInputChannelRChange={(value) => handleChannelChange("input", "right", value)}
      onOutputChannelLChange={(value) => handleChannelChange("output", "left", value)}
      onOutputChannelRChange={(value) => handleChannelChange("output", "right", value)}
      onSampleRateChange={handleSampleRateChange}
      onBufferSizeChange={handleBufferSizeChange}
      onTransmitChannelsChange={handleTransmitChannelsChange}
      diagnosticsState={diagnosticsState}
      diagnosticsProgress={diagnosticsProgress}
      diagnosticsProgressMessage={diagnosticsProgressMessage}
      diagnosticsResult={diagnosticsResult}
      onRunDiagnostics={handleRunDiagnostics}
      onCancelDiagnostics={handleCancelDiagnostics}
      onApplyPreset={handleApplyPreset}
      onOpenLogFolder={handleOpenLogFolder}
      logFolder={logFolder}
      logFolderError={logFolderError}
      usageReporting={usageReporting}
      onUsageReportingChange={handleUsageReportingChange}
      usagePreview={usagePreview}
      onShowUsagePreview={handleShowUsagePreview}
      usagePreviewError={usagePreviewError}
    />
  );
}

export default SettingsPanelAdapter;
