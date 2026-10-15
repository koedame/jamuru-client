import type { Meta, StoryObj } from "@storybook/react-vite";
import { SettingsPanel } from "./SettingsPanel";
import { DeviceInfo } from "./tabs";
import { SelectOption } from "./Select";
import { CompleteDiagnosticsResult } from "../../lib/tauri";

const meta = {
  title: "Windows/Settings",
  component: SettingsPanel,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div style={{ width: "600px", height: "500px" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof SettingsPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

const mockInputDevices: DeviceInfo[] = [
  { id: "mic-1", name: "Built-in Microphone", isDefault: true },
  { id: "mic-2", name: "USB Audio Device" },
  { id: "mic-3", name: "Bluetooth Headset" },
];

const mockOutputDevices: DeviceInfo[] = [
  { id: "out-1", name: "Built-in Speakers", isDefault: true },
  { id: "out-2", name: "USB Audio Device" },
  { id: "out-3", name: "Bluetooth Headset" },
];

// Simulates a stereo device with 2 channels
const stereoChannelOptions: SelectOption[] = [
  { value: "1", label: "Ch 1" },
  { value: "2", label: "Ch 2" },
];

// Simulates a multichannel device (e.g., audio interface with 8 inputs)
const multiChannelOptions: SelectOption[] = [
  { value: "1", label: "Ch 1" },
  { value: "2", label: "Ch 2" },
  { value: "3", label: "Ch 3" },
  { value: "4", label: "Ch 4" },
  { value: "5", label: "Ch 5" },
  { value: "6", label: "Ch 6" },
  { value: "7", label: "Ch 7" },
  { value: "8", label: "Ch 8" },
];

const defaultArgs = {
  language: "ja" as const,
  displayName: "User",
  inputDevices: mockInputDevices,
  outputDevices: mockOutputDevices,
  selectedInputId: "mic-1",
  selectedOutputId: "out-1",
  inputChannelOptions: stereoChannelOptions,
  outputChannelOptions: stereoChannelOptions,
  selectedInputChannelL: "1",
  selectedInputChannelR: "2",
  selectedOutputChannelL: "1",
  selectedOutputChannelR: "2",
  transmitChannelOptions: [
    { value: "1", label: "Mono" },
    { value: "2", label: "Stereo" },
  ],
  selectedTransmitChannels: "2",
  onLanguageChange: () => {},
  onDisplayNameChange: () => {},
  onInputDeviceChange: () => {},
  onOutputDeviceChange: () => {},
  onInputChannelLChange: () => {},
  onInputChannelRChange: () => {},
  onOutputChannelLChange: () => {},
  onOutputChannelRChange: () => {},
};

export const Default: Story = {
  args: {
    ...defaultArgs,
    initialTab: "general",
  },
};

export const GeneralTab: Story = {
  args: {
    ...defaultArgs,
    initialTab: "general",
  },
};

export const DevicesTab: Story = {
  args: {
    ...defaultArgs,
    initialTab: "devices",
  },
};

export const PrivacyTab: Story = {
  args: {
    ...defaultArgs,
    initialTab: "privacy",
    usageReporting: false,
    onUsageReportingChange: () => {},
  },
};

export const AboutTab: Story = {
  args: {
    ...defaultArgs,
    initialTab: "about",
    legal: {
      onOpenTerms: () => {},
      onOpenLicense: () => {},
      onOpenPrivacy: () => {},
      onOpenAnnouncements: () => {},
    },
  },
};

export const DiagnosticsTab: Story = {
  args: {
    ...defaultArgs,
    initialTab: "diagnostics",
    diagnosticsState: "idle",
    onRunDiagnostics: () => alert("Running diagnostics..."),
  },
};

/** Diagnostics running state with progress */
export const DiagnosticsRunning: Story = {
  args: {
    ...defaultArgs,
    initialTab: "diagnostics",
    diagnosticsState: "running",
    diagnosticsProgress: 45,
    diagnosticsProgressMessage: "Checking audio devices",
    onCancelDiagnostics: () => {},
  },
};

// Mock diagnostics result
const mockDiagnosticsResult: CompleteDiagnosticsResult = {
  overall_score: 85,
  recommended_preset: "UltraLowLatency",
  zero_latency_compatible: true,
  network: {
    ip_support: {
      ipv4_available: true,
      ipv6_available: true,
      ipv4_addresses: ["192.168.1.100"],
      ipv6_addresses: ["fe80::1"],
      public_ipv4: "203.0.113.50",
      public_ipv6: null,
    },
    nat_type: "FullCone",
    connection_stability: "A",
    jitter_ms: 5.2,
    stability_metrics: {
      avg_rtt_ms: 18.5,
      min_rtt_ms: 12.0,
      max_rtt_ms: 25.0,
      successful_probes: 10,
      failed_probes: 0,
      packet_loss_rate: 0.0,
    },
    signaling: {
      connected: true,
      connection_time_ms: 150,
      error: null,
    },
    problems: [],
  },
  audio: {
    input_devices: [
      {
        id: "mic-1",
        name: "Built-in Microphone",
        is_default: true,
        is_asio: false,
        supported_sample_rates: [44100, 48000, 96000],
        supports_48khz: true,
        supported_channels: [1, 2],
        grade: "B",
      },
    ],
    output_devices: [
      {
        id: "out-1",
        name: "Built-in Speakers",
        is_default: true,
        is_asio: false,
        supported_sample_rates: [44100, 48000, 96000],
        supports_48khz: true,
        supported_channels: [2],
        grade: "B",
      },
    ],
    selected_input: {
      id: "mic-1",
      name: "Built-in Microphone",
      is_default: true,
      is_asio: false,
      supported_sample_rates: [44100, 48000, 96000],
      supports_48khz: true,
      supported_channels: [1, 2],
      grade: "B",
    },
    selected_output: {
      id: "out-1",
      name: "Built-in Speakers",
      is_default: true,
      is_asio: false,
      supported_sample_rates: [44100, 48000, 96000],
      supports_48khz: true,
      supported_channels: [2],
      grade: "B",
    },
    input_source: "OsDefault",
    output_source: "OsDefault",
    low_latency_support: {
      asio_available: false,
      asio_devices: [],
      supports_32_samples: false,
      supports_64_samples: true,
      supports_128_samples: true,
      min_buffer_size: 64,
      estimated_min_latency_ms: 2.7,
    },
    overall_grade: "B",
    problems: [],
  },
  cpu: {
    benchmarks: [
      {
        buffer_size: 32,
        processing_time_us: 120,
        frame_duration_us: 667,
        realtime_factor: 0.18,
      },
      {
        buffer_size: 64,
        processing_time_us: 180,
        frame_duration_us: 1333,
        realtime_factor: 0.14,
      },
      {
        buffer_size: 128,
        processing_time_us: 280,
        frame_duration_us: 2667,
        realtime_factor: 0.11,
      },
    ],
    system: {
      cpu_cores: 8,
      cpu_usage: 0.15,
      available_memory_mb: 8192,
    },
    grade: "A",
    realtime_capable: true,
    problems: [],
  },
  problems: [],
};

/** Diagnostics complete with results */
export const DiagnosticsComplete: Story = {
  args: {
    ...defaultArgs,
    initialTab: "diagnostics",
    diagnosticsState: "complete",
    diagnosticsResult: mockDiagnosticsResult,
    onRunDiagnostics: () => alert("Running diagnostics..."),
    onApplyPreset: (preset) => alert(`Applying preset: ${preset}`),
  },
};

/** Diagnostics complete with problems */
export const DiagnosticsWithProblems: Story = {
  args: {
    ...defaultArgs,
    initialTab: "diagnostics",
    diagnosticsState: "complete",
    diagnosticsResult: {
      ...mockDiagnosticsResult,
      overall_score: 62,
      zero_latency_compatible: false,
      recommended_preset: "Balanced",
      network: {
        ...mockDiagnosticsResult.network,
        nat_type: "Symmetric",
        connection_stability: "B",
        jitter_ms: 25.0,
        stability_metrics: {
          ...mockDiagnosticsResult.network.stability_metrics,
          avg_rtt_ms: 85.0,
          packet_loss_rate: 0.03,
        },
      },
      problems: [
        {
          severity: "Warning",
          category: "network",
          code: { type: "HighJitter", data: { jitter_ms: 25.0 } },
        },
        {
          severity: "Error",
          category: "network",
          code: { type: "SymmetricNat" },
        },
        {
          severity: "Info",
          category: "audio",
          code: { type: "NoAsioDevices" },
        },
      ],
    },
    onRunDiagnostics: () => alert("Running diagnostics..."),
    onApplyPreset: (preset) => alert(`Applying preset: ${preset}`),
  },
};

export const WithValidationError: Story = {
  args: {
    ...defaultArgs,
    initialTab: "general",
    displayName: "",
    displayNameError: "Name is required",
  },
};

export const Loading: Story = {
  args: {
    ...defaultArgs,
    initialTab: "devices",
    inputDevices: [],
    outputDevices: [],
    selectedInputId: null,
    selectedOutputId: null,
    isLoading: true,
  },
};

/** Audio interface with 8 input/output channels */
export const MultiChannelDevice: Story = {
  args: {
    ...defaultArgs,
    initialTab: "devices",
    inputChannelOptions: multiChannelOptions,
    outputChannelOptions: multiChannelOptions,
    selectedInputChannelL: "3",
    selectedInputChannelR: "4",
    selectedOutputChannelL: "1",
    selectedOutputChannelR: "2",
  },
};

/** Mono configuration: R channel set to None */
export const MonoConfiguration: Story = {
  args: {
    ...defaultArgs,
    initialTab: "devices",
    selectedInputChannelL: "1",
    selectedInputChannelR: null,
    selectedOutputChannelL: "1",
    selectedOutputChannelR: null,
  },
};

/** Mixed configuration: Input is mono, Output is stereo */
export const MonoInputStereoOutput: Story = {
  args: {
    ...defaultArgs,
    initialTab: "devices",
    selectedInputChannelL: "1",
    selectedInputChannelR: null,
    selectedOutputChannelL: "1",
    selectedOutputChannelR: "2",
  },
};

/** 8-channel interface with mono input and stereo output on channels 5-6 */
export const MultiChannelMono: Story = {
  args: {
    ...defaultArgs,
    initialTab: "devices",
    inputChannelOptions: multiChannelOptions,
    outputChannelOptions: multiChannelOptions,
    selectedInputChannelL: "3",
    selectedInputChannelR: null,
    selectedOutputChannelL: "5",
    selectedOutputChannelR: "6",
  },
};
