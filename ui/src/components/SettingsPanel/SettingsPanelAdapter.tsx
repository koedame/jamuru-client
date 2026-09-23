/**
 * SettingsPanelAdapter - Wraps Storybook SettingsPanel with Tauri API
 * Connects Props-based SettingsPanel to Tauri backend
 */
import { useState, useEffect, useCallback, useRef } from "react";
import { useTranslation } from "react-i18next";
import {
  AudioDeviceInfo,
  audioListInputDevices,
  audioListOutputDevices,
  audioSetInputDevice,
  audioSetOutputDevice,
  audioGetCurrentDevices,
  audioGetBufferSize,
  audioSetBufferSize,
  audioGetDeviceChannels,
  streamingStatus,
  streamingStart,
  streamingStop,
  streamingSetInputDevice,
  streamingSetOutputDevice,
  configLoad,
  configSave,
  configGetPeerName,
  configSetPeerName,
  configGetServerUrl,
  configSetServerUrl,
  configGetEffectiveServerUrl,
  configGetSampleRate,
  configSetSampleRate,
  configListSampleRates,
  configGetInputChannels,
  configSetInputChannels,
  configGetOutputChannels,
  configSetOutputChannels,
  configGetTransmitChannels,
  configSetTransmitChannels,
  configSetLanguage,
  diagnosticsRunComplete,
  type AppConfig,
  type SampleRateInfo,
  type CompleteDiagnosticsResult,
  type RecommendedPreset,
  logOpenDir,
  usagePreview as readUsagePreview,
} from "../../lib/tauri";
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

export function SettingsPanelAdapter({
  initialTab = "general",
  onSettingsChange,
}: SettingsPanelAdapterProps) {
  const { i18n, t } = useTranslation();

  // State
  const [inputDevices, setInputDevices] = useState<DeviceInfo[]>([]);
  const [outputDevices, setOutputDevices] = useState<DeviceInfo[]>([]);
  const [selectedInputId, setSelectedInputId] = useState<string | null>(null);
  const [selectedOutputId, setSelectedOutputId] = useState<string | null>(null);
  const [bufferSize, setBufferSize] = useState<number>(64);
  const [sampleRate, setSampleRate] = useState<number>(48000);
  const [sampleRateInfos, setSampleRateInfos] = useState<SampleRateInfo[]>([]);
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

  // Channel selection state
  const [inputChannelL, setInputChannelL] = useState<string>("1");
  const [inputChannelR, setInputChannelR] = useState<string>("2");
  const [outputChannelL, setOutputChannelL] = useState<string>("1");
  const [outputChannelR, setOutputChannelR] = useState<string>("2");
  const [transmitChannels, setTransmitChannels] = useState<string>("2");
  const [inputChannelCount, setInputChannelCount] = useState(2);
  const [outputChannelCount, setOutputChannelCount] = useState(2);

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
    sampleRateInfos.length > 0
      ? sampleRateInfos.map((sr) => ({
          value: String(sr.rate),
          label: sr.recommended ? `${sr.label} (${t("preset.recommended")})` : sr.label,
        }))
      : [{ value: "48000", label: "48 kHz" }];

  // Buffer size options (samples, and their duration at 48 kHz)
  const bufferSizeOptions: SelectOption[] = [
    ["8", "0.17"],
    ["16", "0.33"],
    ["32", "0.67"],
    ["64", "1.33"],
    ["128", "2.67"],
    ["256", "5.33"],
  ].map(([samples, ms]) => ({
    value: samples,
    label: t("settings.devices.bufferOption", { samples, ms }),
  }));

  // Transmit channel options
  const transmitChannelOptions: SelectOption[] = [
    { value: "1", label: t("settings.devices.mono", "Mono") },
    { value: "2", label: t("settings.devices.stereo", "Stereo") },
  ];

  // Helper to build channel options from device channel count
  const buildChannelOptions = (maxChannels: number): SelectOption[] => {
    const options: SelectOption[] = [];
    for (let i = 1; i <= maxChannels; i++) {
      options.push({ value: String(i), label: t("settings.devices.channelOption", { channel: i }) });
    }
    return options.length > 0 ? options : buildChannelOptions(2);
  };

  // Helper to load the channel count of a device
  const loadDeviceChannels = async (deviceId: string, isInput: boolean): Promise<number> => {
    try {
      const channels = await audioGetDeviceChannels(deviceId, isInput);
      return channels.length > 0 ? Math.max(...channels) : 2;
    } catch {
      // Default to stereo if we can't get device info
      return 2;
    }
  };

  // Load settings on mount
  useEffect(() => {
    const loadSettings = async () => {
      try {
        setIsLoading(true);

        const [
          inputs,
          outputs,
          current,
          currentBufferSize,
          savedPeerName,
          savedServerUrl,
          currentEffectiveServerUrl,
          currentSampleRate,
          sampleRates,
          inputChannelConfig,
          outputChannelConfig,
          transmitChannelConfig,
        ] = await Promise.all([
          audioListInputDevices(),
          audioListOutputDevices(),
          audioGetCurrentDevices(),
          audioGetBufferSize(),
          configGetPeerName().catch(() => "User"),
          configGetServerUrl().catch(() => null),
          configGetEffectiveServerUrl().catch(() => ""),
          configGetSampleRate().catch(() => 48000),
          configListSampleRates().catch(() => [] as SampleRateInfo[]),
          configGetInputChannels().catch(() => ({ channel_l: 1, channel_r: 2 })),
          configGetOutputChannels().catch(() => ({ channel_l: 1, channel_r: 2 })),
          configGetTransmitChannels().catch(() => 2),
        ]);

        setInputDevices(inputs.map(toDeviceInfo));
        setOutputDevices(outputs.map(toDeviceInfo));
        setDisplayName(savedPeerName);
        setServerUrl(savedServerUrl ?? "");
        setEffectiveServerUrl(currentEffectiveServerUrl);
        setBufferSize(currentBufferSize);
        setSampleRate(currentSampleRate);

        // Set channel selections from config
        setInputChannelL(String(inputChannelConfig.channel_l));
        setInputChannelR(inputChannelConfig.channel_r ? String(inputChannelConfig.channel_r) : "2");
        setOutputChannelL(String(outputChannelConfig.channel_l));
        setOutputChannelR(outputChannelConfig.channel_r ? String(outputChannelConfig.channel_r) : "2");
        setTransmitChannels(String(transmitChannelConfig));

        // Convert sample rate info to select options
        setSampleRateInfos(sampleRates);

        // Set selected devices or use defaults
        let inputId = current.input_device_id;
        let outputId = current.output_device_id;

        if (!inputId && inputs.length > 0) {
          const defaultInput = inputs.find((d) => d.is_default) || inputs[0];
          inputId = defaultInput.id;
          await audioSetInputDevice(inputId);
        }

        if (!outputId && outputs.length > 0) {
          const defaultOutput = outputs.find((d) => d.is_default) || outputs[0];
          outputId = defaultOutput.id;
          await audioSetOutputDevice(outputId);
        }

        setSelectedInputId(inputId);
        setSelectedOutputId(outputId);

        // Load channel options for selected devices
        if (inputId) {
          setInputChannelCount(await loadDeviceChannels(inputId, true));
        }

        if (outputId) {
          setOutputChannelCount(await loadDeviceChannels(outputId, false));
        }
      } catch (err) {
        console.error("Failed to load settings:", err);
      } finally {
        setIsLoading(false);
      }
    };

    loadSettings();
  }, []);

  useEffect(() => {
    configLoad()
      .then((config) => setUsageReporting(config?.usage_reporting === true))
      .catch((err) => console.error("Failed to read the usage reporting setting:", err));
  }, []);

  // Save config helper
  const saveConfig = async (
    inputId: string | null,
    outputId: string | null,
    bufSize: number
  ) => {
    try {
      const config = await configLoad().catch(
        () =>
          ({
            input_device_id: null,
            output_device_id: null,
            buffer_size: 64,
            server_url: null,
          } as AppConfig)
      );

      await configSave({
        ...config,
        input_device_id: inputId,
        output_device_id: outputId,
        buffer_size: bufSize,
      });
    } catch (e) {
      console.error("Failed to save config:", e);
    }
  };

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
    async (deviceId: string) => {
      try {
        await audioSetInputDevice(deviceId);
        setSelectedInputId(deviceId);
        await saveConfig(deviceId, selectedOutputId, bufferSize);

        // Update channel options for new device
        const maxChannel = await loadDeviceChannels(deviceId, true);
        setInputChannelCount(maxChannel);

        // Reset channel selections if they exceed new device's channels
        if (parseInt(inputChannelL, 10) > maxChannel) {
          setInputChannelL("1");
          await configSetInputChannels(1, parseInt(inputChannelR, 10) <= maxChannel ? parseInt(inputChannelR, 10) : null);
        }
        if (parseInt(inputChannelR, 10) > maxChannel) {
          setInputChannelR(String(Math.min(2, maxChannel)));
          await configSetInputChannels(parseInt(inputChannelL, 10), Math.min(2, maxChannel));
        }

        // Update running stream if active
        try {
          const status = await streamingStatus();
          if (status.is_active) {
            await streamingSetInputDevice(deviceId);
          }
        } catch {
          // Ignore
        }
      } catch (err) {
        console.error("Failed to set input device:", err);
      }
    },
    [selectedOutputId, bufferSize, inputChannelL, inputChannelR]
  );

  const handleOutputDeviceChange = useCallback(
    async (deviceId: string) => {
      try {
        await audioSetOutputDevice(deviceId);
        setSelectedOutputId(deviceId);
        await saveConfig(selectedInputId, deviceId, bufferSize);

        // Update channel options for new device
        const maxChannel = await loadDeviceChannels(deviceId, false);
        setOutputChannelCount(maxChannel);

        // Reset channel selections if they exceed new device's channels
        if (parseInt(outputChannelL, 10) > maxChannel) {
          setOutputChannelL("1");
          await configSetOutputChannels(1, parseInt(outputChannelR, 10) <= maxChannel ? parseInt(outputChannelR, 10) : null);
        }
        if (parseInt(outputChannelR, 10) > maxChannel) {
          setOutputChannelR(String(Math.min(2, maxChannel)));
          await configSetOutputChannels(parseInt(outputChannelL, 10), Math.min(2, maxChannel));
        }

        // Update running stream if active
        try {
          const status = await streamingStatus();
          if (status.is_active) {
            await streamingSetOutputDevice(deviceId);
          }
        } catch {
          // Ignore
        }
      } catch (err) {
        console.error("Failed to set output device:", err);
      }
    },
    [selectedInputId, bufferSize, outputChannelL, outputChannelR]
  );

  const handleBufferSizeChange = useCallback(
    async (value: string) => {
      const newSize = parseInt(value, 10);
      if (isNaN(newSize)) return;

      try {
        await audioSetBufferSize(newSize);
        setBufferSize(newSize);
        await saveConfig(selectedInputId, selectedOutputId, newSize);

        // Restart streaming if active
        try {
          const status = await streamingStatus();
          if (status.is_active && status.remote_addr) {
            await streamingStop();
            await new Promise((resolve) => setTimeout(resolve, 100));
            await streamingStart(
              status.remote_addr,
              undefined,
              selectedInputId ?? undefined,
              selectedOutputId ?? undefined,
              newSize
            );
          }
        } catch {
          // Ignore
        }
      } catch (err) {
        console.error("Failed to set buffer size:", err);
      }
    },
    [selectedInputId, selectedOutputId]
  );

  const handleSampleRateChange = useCallback(
    async (value: string) => {
      const newRate = parseInt(value, 10);
      if (isNaN(newRate)) return;

      try {
        await configSetSampleRate(newRate);
        setSampleRate(newRate);
        onSettingsChange?.();
      } catch (err) {
        console.error("Failed to set sample rate:", err);
      }
    },
    [onSettingsChange]
  );

  // Channel change handlers
  const handleInputChannelLChange = useCallback(
    async (value: string) => {
      const channel = parseInt(value, 10);
      if (isNaN(channel)) return;

      try {
        const channelR = inputChannelR ? parseInt(inputChannelR, 10) : null;
        await configSetInputChannels(channel, channelR);
        setInputChannelL(value);
        onSettingsChange?.();
      } catch (err) {
        console.error("Failed to set input channel L:", err);
      }
    },
    [inputChannelR, onSettingsChange]
  );

  const handleInputChannelRChange = useCallback(
    async (value: string) => {
      const channel = parseInt(value, 10);
      if (isNaN(channel)) return;

      try {
        const channelL = parseInt(inputChannelL, 10);
        await configSetInputChannels(channelL, channel);
        setInputChannelR(value);
        onSettingsChange?.();
      } catch (err) {
        console.error("Failed to set input channel R:", err);
      }
    },
    [inputChannelL, onSettingsChange]
  );

  const handleOutputChannelLChange = useCallback(
    async (value: string) => {
      const channel = parseInt(value, 10);
      if (isNaN(channel)) return;

      try {
        const channelR = outputChannelR ? parseInt(outputChannelR, 10) : null;
        await configSetOutputChannels(channel, channelR);
        setOutputChannelL(value);
        onSettingsChange?.();
      } catch (err) {
        console.error("Failed to set output channel L:", err);
      }
    },
    [outputChannelR, onSettingsChange]
  );

  const handleOutputChannelRChange = useCallback(
    async (value: string) => {
      const channel = parseInt(value, 10);
      if (isNaN(channel)) return;

      try {
        const channelL = parseInt(outputChannelL, 10);
        await configSetOutputChannels(channelL, channel);
        setOutputChannelR(value);
        onSettingsChange?.();
      } catch (err) {
        console.error("Failed to set output channel R:", err);
      }
    },
    [outputChannelL, onSettingsChange]
  );

  const handleTransmitChannelsChange = useCallback(
    async (value: string) => {
      const count = parseInt(value, 10);
      if (isNaN(count) || (count !== 1 && count !== 2)) return;

      try {
        await configSetTransmitChannels(count);
        setTransmitChannels(value);
        onSettingsChange?.();
      } catch (err) {
        console.error("Failed to set transmit channels:", err);
      }
    },
    [onSettingsChange]
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
        const config = await configLoad();
        await configSave({ ...config, usage_reporting: enabled });
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
    async (preset: RecommendedPreset) => {
      // Apply preset settings based on recommendation
      // This maps presets to buffer size and other settings
      const presetSettings: Record<RecommendedPreset, { bufferSize: number }> = {
        ZeroLatency: { bufferSize: 8 },
        UltraLowLatency: { bufferSize: 32 },
        Balanced: { bufferSize: 64 },
        HighQuality: { bufferSize: 128 },
      };

      const settings = presetSettings[preset];
      if (settings) {
        try {
          await audioSetBufferSize(settings.bufferSize);
          setBufferSize(settings.bufferSize);
          await saveConfig(selectedInputId, selectedOutputId, settings.bufferSize);
          onSettingsChange?.();
        } catch (err) {
          console.error("Failed to apply preset:", err);
        }
      }
    },
    [selectedInputId, selectedOutputId, onSettingsChange]
  );

  return (
    <SettingsPanel
      initialTab={initialTab}
      language={language}
      serverUrl={serverUrl}
      effectiveServerUrl={effectiveServerUrl}
      displayName={displayName}
      displayNameError={displayNameError}
      inputDevices={inputDevices}
      outputDevices={outputDevices}
      selectedInputId={selectedInputId}
      selectedOutputId={selectedOutputId}
      inputChannelOptions={buildChannelOptions(inputChannelCount)}
      outputChannelOptions={buildChannelOptions(outputChannelCount)}
      selectedInputChannelL={inputChannelL}
      selectedInputChannelR={inputChannelR}
      selectedOutputChannelL={outputChannelL}
      selectedOutputChannelR={outputChannelR}
      sampleRateOptions={sampleRateOptions}
      selectedSampleRate={String(sampleRate)}
      bufferSizeOptions={bufferSizeOptions}
      selectedBufferSize={String(bufferSize)}
      transmitChannelOptions={transmitChannelOptions}
      selectedTransmitChannels={transmitChannels}
      isLoading={isLoading}
      onLanguageChange={handleLanguageChange}
      onServerUrlChange={handleServerUrlChange}
      onDisplayNameChange={handleDisplayNameChange}
      onInputDeviceChange={handleInputDeviceChange}
      onOutputDeviceChange={handleOutputDeviceChange}
      onInputChannelLChange={handleInputChannelLChange}
      onInputChannelRChange={handleInputChannelRChange}
      onOutputChannelLChange={handleOutputChannelLChange}
      onOutputChannelRChange={handleOutputChannelRChange}
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
