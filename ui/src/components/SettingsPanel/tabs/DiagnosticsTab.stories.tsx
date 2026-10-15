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
    logFolder: "/home/user/.local/share/me.koeda.jamuru/logs",
  },
};

export const LogFolderError: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    onOpenLogFolder: () => {},
    logFolderError:
      "Could not open the log folder /home/user/.local/share/me.koeda.jamuru/logs: No such file or directory",
  },
};

export const ReportProblemIdle: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    reportProblemState: "idle",
    onOpenReportProblem: () => {},
  },
};

export const ReportProblemReady: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    reportProblemState: "ready",
    reportProblemPreview: "[2026-09-27T12:00:00Z INFO jamuru] starting\n[2026-09-27T12:00:03Z WARN jamuru] no_packets from peer",
    reportProblemComment: "音が届かなくなりました",
    onOpenReportProblem: () => {},
    onReportProblemCommentChange: () => {},
    onSendReportProblem: () => {},
    onCancelReportProblem: () => {},
  },
};

export const ReportProblemSent: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    reportProblemState: "sent",
    onOpenReportProblem: () => {},
    onCancelReportProblem: () => {},
  },
};

export const ReportProblemError: Story = {
  args: {
    state: "idle",
    onRunDiagnostics: () => {},
    reportProblemState: "error",
    reportProblemPreview: "[2026-09-27T12:00:00Z INFO jamuru] starting",
    reportProblemError: "送信できませんでした。しばらくしてからもう一度試してください",
    onOpenReportProblem: () => {},
    onReportProblemCommentChange: () => {},
    onSendReportProblem: () => {},
    onCancelReportProblem: () => {},
  },
};
