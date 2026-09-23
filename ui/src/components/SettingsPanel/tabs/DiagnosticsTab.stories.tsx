import type { Meta, StoryObj } from "@storybook/react-vite";
import { DiagnosticsTab } from "./DiagnosticsTab";
import type { CompleteDiagnosticsResult } from "../../../lib/tauri";

const meta = {
  title: "Components/Settings/Tabs/DiagnosticsTab",
  component: DiagnosticsTab,
  parameters: {
    layout: "centered",
  },
  decorators: [
    (Story) => (
      <div style={{ width: "450px", background: "var(--color-bg-primary)", padding: "16px" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof DiagnosticsTab>;

export default meta;
type Story = StoryObj<typeof meta>;

const goodResult: CompleteDiagnosticsResult = {
  network: {
    ip_support: {
      ipv4_available: true,
      ipv6_available: true,
      ipv4_addresses: ["192.168.1.10"],
      ipv6_addresses: [],
      public_ipv4: "203.0.113.10",
      public_ipv6: null,
    },
    nat_type: "FullCone",
    connection_stability: "A",
    jitter_ms: 3,
    stability_metrics: {
      avg_rtt_ms: 15,
      min_rtt_ms: 10,
      max_rtt_ms: 22,
      successful_probes: 20,
      failed_probes: 0,
      packet_loss_rate: 0,
    },
    signaling: { connected: true, connection_time_ms: 120, error: null },
    problems: [],
  },
  audio: {
    input_devices: [],
    output_devices: [],
    selected_input: {
      id: "default-in",
      name: "MacBook Pro Microphone",
      is_default: true,
      is_asio: false,
      supported_sample_rates: [48000],
      supports_48khz: true,
      supported_channels: [1, 2],
      grade: "A",
    },
    selected_output: {
      id: "default-out",
      name: "MacBook Pro Speakers",
      is_default: true,
      is_asio: false,
      supported_sample_rates: [48000],
      supports_48khz: true,
      supported_channels: [1, 2],
      grade: "A",
    },
    input_source: "OsDefault",
    output_source: "OsDefault",
    low_latency_support: {
      asio_available: false,
      asio_devices: [],
      supports_32_samples: true,
      supports_64_samples: true,
      supports_128_samples: true,
      min_buffer_size: 32,
      estimated_min_latency_ms: 0.67,
    },
    overall_grade: "A",
    problems: [],
  },
  cpu: {
    benchmarks: [{ processing_time_us: 120, frame_duration_us: 1333, realtime_factor: 0.09, buffer_size: 64 }],
    system: { cpu_cores: 10, cpu_usage: 0.15, available_memory_mb: 8192 },
    grade: "A",
    realtime_capable: true,
    problems: [],
  },
  overall_score: 96,
  recommended_preset: "Balanced",
  zero_latency_compatible: true,
  problems: [],
};

const problemResult: CompleteDiagnosticsResult = {
  ...goodResult,
  network: {
    ...goodResult.network,
    connection_stability: "C",
    nat_type: "Symmetric",
    problems: [
      {
        severity: "Warning",
        category: "network",
        code: { type: "SymmetricNat" },
      },
    ],
  },
  overall_score: 58,
  zero_latency_compatible: false,
  problems: [
    {
      severity: "Warning",
      category: "network",
      code: { type: "SymmetricNat" },
    },
    {
      severity: "Error",
      category: "audio",
      code: { type: "InputNot48kHz", data: { device_name: "USB Microphone" } },
    },
  ],
};

export const Idle: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
  },
};

export const Running: Story = {
  args: {
    state: "running",
    progress: 45,
    progressMessage: "オーディオデバイスを確認中...",
    onCancelDiagnostics: () => {},
  },
};

export const CompleteGood: Story = {
  args: {
    state: "complete",
    result: goodResult,
    onRunDiagnostics: () => {},
    onApplyPreset: () => {},
  },
};

export const CompleteWithProblems: Story = {
  args: {
    state: "complete",
    result: problemResult,
    onRunDiagnostics: () => {},
    onApplyPreset: () => {},
  },
};

export const IdleWithLogFile: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    onOpenLogFolder: () => {},
  },
};

export const LogFolderOpened: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    onOpenLogFolder: () => {},
    logFolder: "/home/user/.local/share/me.koeda.jamjam/logs",
  },
};

export const LogFolderError: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    onOpenLogFolder: () => {},
    logFolderError:
      "Could not open the log folder /home/user/.local/share/me.koeda.jamjam/logs: No such file or directory",
  },
};

export const UsageReportingOff: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    onOpenLogFolder: () => {},
    usageReporting: false,
    onUsageReportingChange: () => {},
    onShowUsagePreview: () => {},
  },
};

export const UsageReportingOn: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    onOpenLogFolder: () => {},
    usageReporting: true,
    onUsageReportingChange: () => {},
    onShowUsagePreview: () => {},
  },
};

export const UsageReportingPreview: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    onOpenLogFolder: () => {},
    usageReporting: true,
    onUsageReportingChange: () => {},
    onShowUsagePreview: () => {},
    usagePreview: [
      '{"v":1,"ts":"2026-09-24T02:10:00Z","seq":1,"event":"app_start","install_id":"a91d5c0e7b3f4a68b2c1d0e9f8a7b6c5","launch_id":"7f3c9a1e2b4d4c6f8e0a1b2c3d4e5f60","session_id":null,"app_version":"0.1.0","os":"macos","arch":"aarch64","os_version":"14.6","cpu_cores":8,"ram_gb":16,"language":"ja","audio_host":"coreaudio","settings":{"preset":"low_latency","sample_rate":48000,"buffer_size":128,"transmit_channels":2}}',
      '{"v":1,"ts":"2026-09-24T02:10:00Z","seq":2,"event":"audio_env","install_id":"a91d5c0e7b3f4a68b2c1d0e9f8a7b6c5","launch_id":"7f3c9a1e2b4d4c6f8e0a1b2c3d4e5f60","session_id":null,"app_version":"0.1.0","os":"macos","arch":"aarch64","input":{"name":"Scarlett 2i2 USB","kind":"usb","channels":2,"sample_rates":[44100,48000,96000],"min_buffer_frames":32,"is_default":true},"output":{"name":"Taro\'s AirPods","kind":"bluetooth","channels":2,"sample_rates":[48000],"min_buffer_frames":128,"is_default":false}}',
    ].join("\n"),
  },
};

export const UsageReportingOffPreview: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    onOpenLogFolder: () => {},
    usageReporting: false,
    onUsageReportingChange: () => {},
    onShowUsagePreview: () => {},
    usagePreview: "",
  },
};
