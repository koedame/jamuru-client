/**
 * SettingsPanel - Settings panel with header and vertical tabs
 *
 * Design: jamjam brand (ui.pen Screens/Settings). Header bar + left sidebar of
 * icon tabs (Devices / General / Profile / Diagnostics) + content area.
 */

import { useState, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { VerticalTabs, Tab } from "./VerticalTabs";
import { GeneralTab, Language } from "./tabs/GeneralTab";
import { ProfileTab } from "./tabs/ProfileTab";
import { DevicesTab, DeviceInfo } from "./tabs/DevicesTab";
import { DiagnosticsTab, DiagnosticsState } from "./tabs/DiagnosticsTab";
import { SelectOption } from "./Select";
import { MicIcon, SlidersIcon, UserIcon, ActivityIcon } from "./icons";
import { CompleteDiagnosticsResult, RecommendedPreset } from "../../lib/tauri";
import "./SettingsPanel.css";

export type SettingsTabId = "devices" | "general" | "profile" | "diagnostics";

export interface SettingsPanelProps {
  /** Initial tab */
  initialTab?: SettingsTabId;
  /** Header title (defaults to i18n settings.title) */
  title?: string;
  /** Language */
  language: Language;
  /** Signaling server override as stored in config (empty = use the build default) */
  serverUrl?: string;
  /** The URL the app will actually dial right now (the override, or the build default) */
  effectiveServerUrl?: string;
  /** Display name */
  displayName: string;
  /** Display name error */
  displayNameError?: string;
  /** Input devices */
  inputDevices: DeviceInfo[];
  /** Output devices */
  outputDevices: DeviceInfo[];
  /** Selected input device ID */
  selectedInputId: string | null;
  /** Selected output device ID */
  selectedOutputId: string | null;
  /** Input channel options (e.g., Ch 1, Ch 2, etc.) */
  inputChannelOptions?: SelectOption[];
  /** Output channel options (e.g., Ch 1, Ch 2, etc.) */
  outputChannelOptions?: SelectOption[];
  /** Selected input L/MONO channel */
  selectedInputChannelL?: string;
  /** Selected input R channel (null or empty = mono) */
  selectedInputChannelR?: string | null;
  /** Selected output L/MONO channel */
  selectedOutputChannelL?: string;
  /** Selected output R channel (null or empty = mono) */
  selectedOutputChannelR?: string | null;
  /** Sample rate options */
  sampleRateOptions?: SelectOption[];
  /** Selected sample rate */
  selectedSampleRate?: string;
  /** Buffer size options */
  bufferSizeOptions?: SelectOption[];
  /** Selected buffer size */
  selectedBufferSize?: string;
  /** Transmit channel options */
  transmitChannelOptions?: SelectOption[];
  /** Selected transmit channels */
  selectedTransmitChannels?: string;
  /** Loading state */
  isLoading?: boolean;
  /** Diagnostics state */
  diagnosticsState?: DiagnosticsState;
  /** Diagnostics progress (0-100) */
  diagnosticsProgress?: number;
  /** Diagnostics progress message */
  diagnosticsProgressMessage?: string;
  /** Diagnostics result */
  diagnosticsResult?: CompleteDiagnosticsResult;
  /** Language change handler */
  onLanguageChange: (language: Language) => void;
  /** Server URL change handler. Called with an empty string to clear the override. */
  onServerUrlChange?: (url: string) => void;
  /** Display name change handler */
  onDisplayNameChange: (name: string) => void;
  /** Input device change handler */
  onInputDeviceChange: (id: string) => void;
  /** Output device change handler */
  onOutputDeviceChange: (id: string) => void;
  /** Input L channel change handler */
  onInputChannelLChange?: (value: string) => void;
  /** Input R channel change handler */
  onInputChannelRChange?: (value: string) => void;
  /** Output L channel change handler */
  onOutputChannelLChange?: (value: string) => void;
  /** Output R channel change handler */
  onOutputChannelRChange?: (value: string) => void;
  /** Sample rate change handler */
  onSampleRateChange?: (value: string) => void;
  /** Buffer size change handler */
  onBufferSizeChange?: (value: string) => void;
  /** Transmit channels change handler */
  onTransmitChannelsChange?: (value: string) => void;
  /** Run diagnostics handler */
  onRunDiagnostics?: () => void;
  /** Cancel diagnostics handler (shows a cancel button while running) */
  onCancelDiagnostics?: () => void;
  /** Apply recommended preset handler */
  onApplyPreset?: (preset: RecommendedPreset) => void;
  /** Open the folder holding the log file */
  onOpenLogFolder?: () => void;
  /** Folder that was opened */
  logFolder?: string | null;
  /** Why the folder could not be opened */
  logFolderError?: string | null;
  /** Whether usage reporting is on */
  usageReporting?: boolean;
  /** Turn usage reporting on or off */
  onUsageReportingChange?: (enabled: boolean) => void;
  /** The lines the next report will contain (null = not shown) */
  usagePreview?: string | null;
  /** Show (or refresh) what would be sent */
  onShowUsagePreview?: () => void;
  /** Why what would be sent could not be read */
  usagePreviewError?: string | null;
}

const DEFAULT_CHANNEL_OPTIONS: SelectOption[] = [
  { value: "1", label: "Ch 1" },
  { value: "2", label: "Ch 2" },
];

const DEFAULT_SAMPLE_RATE_OPTIONS: SelectOption[] = [
  { value: "44100", label: "44.1 kHz" },
  { value: "48000", label: "48 kHz" },
  { value: "96000", label: "96 kHz" },
];

const DEFAULT_BUFFER_SIZE_OPTIONS: SelectOption[] = [
  { value: "32", label: "32 samples" },
  { value: "64", label: "64 samples" },
  { value: "128", label: "128 samples" },
  { value: "256", label: "256 samples" },
];

export function SettingsPanel({
  initialTab = "devices",
  title,
  language,
  serverUrl = "",
  effectiveServerUrl = "",
  displayName,
  displayNameError,
  inputDevices,
  outputDevices,
  selectedInputId,
  selectedOutputId,
  inputChannelOptions = DEFAULT_CHANNEL_OPTIONS,
  outputChannelOptions = DEFAULT_CHANNEL_OPTIONS,
  selectedInputChannelL = "1",
  selectedInputChannelR = "2",
  selectedOutputChannelL = "1",
  selectedOutputChannelR = "2",
  sampleRateOptions = DEFAULT_SAMPLE_RATE_OPTIONS,
  selectedSampleRate = "48000",
  bufferSizeOptions = DEFAULT_BUFFER_SIZE_OPTIONS,
  selectedBufferSize = "64",
  transmitChannelOptions = DEFAULT_CHANNEL_OPTIONS,
  selectedTransmitChannels = "2",
  isLoading,
  diagnosticsState = "idle",
  diagnosticsProgress = 0,
  diagnosticsProgressMessage,
  diagnosticsResult,
  onLanguageChange,
  onServerUrlChange = () => {},
  onDisplayNameChange,
  onInputDeviceChange,
  onOutputDeviceChange,
  onInputChannelLChange = () => {},
  onInputChannelRChange = () => {},
  onOutputChannelLChange = () => {},
  onOutputChannelRChange = () => {},
  onSampleRateChange = () => {},
  onBufferSizeChange = () => {},
  onTransmitChannelsChange = () => {},
  onRunDiagnostics,
  onCancelDiagnostics,
  onApplyPreset,
  onOpenLogFolder,
  logFolder,
  logFolderError,
  usageReporting,
  onUsageReportingChange,
  usagePreview,
  onShowUsagePreview,
  usagePreviewError,
}: SettingsPanelProps) {
  const { t } = useTranslation();
  const [selectedTab, setSelectedTab] = useState<SettingsTabId>(initialTab);

  const tabs: Tab[] = [
    { id: "general", label: t("settings.tabs.general", "General"), icon: <SlidersIcon /> },
    { id: "devices", label: t("settings.tabs.devices", "Devices"), icon: <MicIcon /> },
    { id: "profile", label: t("settings.tabs.profile", "Profile"), icon: <UserIcon /> },
    { id: "diagnostics", label: t("settings.tabs.diagnostics", "Diagnostics"), icon: <ActivityIcon /> },
  ];

  const handleTabSelect = useCallback((id: string) => {
    setSelectedTab(id as SettingsTabId);
  }, []);

  const renderTabContent = () => {
    switch (selectedTab) {
      case "general":
        return (
          <GeneralTab
            language={language}
            onLanguageChange={onLanguageChange}
            serverUrl={serverUrl}
            effectiveServerUrl={effectiveServerUrl}
            onServerUrlChange={onServerUrlChange}
          />
        );
      case "profile":
        return (
          <ProfileTab
            displayName={displayName}
            error={displayNameError}
            onDisplayNameChange={onDisplayNameChange}
          />
        );
      case "devices":
        return (
          <DevicesTab
            inputDevices={inputDevices}
            outputDevices={outputDevices}
            selectedInputId={selectedInputId}
            selectedOutputId={selectedOutputId}
            inputChannelOptions={inputChannelOptions}
            outputChannelOptions={outputChannelOptions}
            selectedInputChannelL={selectedInputChannelL}
            selectedInputChannelR={selectedInputChannelR}
            selectedOutputChannelL={selectedOutputChannelL}
            selectedOutputChannelR={selectedOutputChannelR}
            sampleRateOptions={sampleRateOptions}
            selectedSampleRate={selectedSampleRate}
            bufferSizeOptions={bufferSizeOptions}
            selectedBufferSize={selectedBufferSize}
            transmitChannelOptions={transmitChannelOptions}
            selectedTransmitChannels={selectedTransmitChannels}
            isLoading={isLoading}
            onInputDeviceChange={onInputDeviceChange}
            onOutputDeviceChange={onOutputDeviceChange}
            onInputChannelLChange={onInputChannelLChange}
            onInputChannelRChange={onInputChannelRChange}
            onOutputChannelLChange={onOutputChannelLChange}
            onOutputChannelRChange={onOutputChannelRChange}
            onSampleRateChange={onSampleRateChange}
            onBufferSizeChange={onBufferSizeChange}
            onTransmitChannelsChange={onTransmitChannelsChange}
          />
        );
      case "diagnostics":
        return (
          <DiagnosticsTab
            state={diagnosticsState}
            progress={diagnosticsProgress}
            progressMessage={diagnosticsProgressMessage}
            result={diagnosticsResult}
            onRunDiagnostics={onRunDiagnostics}
            onCancelDiagnostics={onCancelDiagnostics}
            onApplyPreset={onApplyPreset}
            onOpenLogFolder={onOpenLogFolder}
            logFolder={logFolder}
            logFolderError={logFolderError}
            usageReporting={usageReporting}
            onUsageReportingChange={onUsageReportingChange}
            usagePreview={usagePreview}
            onShowUsagePreview={onShowUsagePreview}
            usagePreviewError={usagePreviewError}
          />
        );
      default:
        return null;
    }
  };

  return (
    <div className="settings-panel" data-testid="settings-panel">
      <header className="settings-panel__header">
        <span className="settings-panel__title">{title ?? t("settings.title", "Settings")}</span>
      </header>
      <div className="settings-panel__body">
        <VerticalTabs
          tabs={tabs}
          selectedId={selectedTab}
          onSelect={handleTabSelect}
        />
        <div
          className="settings-panel__content"
          role="tabpanel"
          id={`tabpanel-${selectedTab}`}
          aria-labelledby={`tab-${selectedTab}`}
        >
          {renderTabContent()}
        </div>
      </div>
    </div>
  );
}

export default SettingsPanel;
