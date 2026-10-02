import type { Meta, StoryObj } from "@storybook/react-vite";
import { SessionStats } from "./SessionStats";
import type {
  NetworkStats,
  DetailedLatency,
  PeerAudioInfo,
} from "../lib/tauri";

const meta = {
  title: "Components/Mixer/SessionStats",
  component: SessionStats,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div style={{ width: "400px" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof SessionStats>;

export default meta;
type Story = StoryObj<typeof meta>;

// Sample data
const goodNetworkStats: NetworkStats = {
  rtt_ms: 15.2,
  jitter_ms: 2.1,
  packet_loss_percent: 0.1,
  quality: 'good',
  measured_bps: 3_200_000,
  required_bps: 3_100_000,
  bandwidth_status: 'sufficient',
  uptime_seconds: 3600,
  packets_sent: 54000,
  packets_received: 53950,
  encryption: 'encrypted',
  peer_checked: true,
  bytes_sent: 12582912,
  bytes_received: 12345678,
};

const goodLatency: DetailedLatency = {
  upstream: [
    { name: "Audio capture", ms: 1.33, info: "64 samples @ 48000 Hz" },
    { name: "Encoding", ms: 0.15, info: null },
    { name: "Network (half RTT)", ms: 7.6, info: null },
  ],
  upstream_total_ms: 9.08,
  downstream: [
    { name: "Network (half RTT)", ms: 7.6, info: null },
    { name: "Jitter buffer", ms: 2.67, info: "128 samples @ 48000 Hz" },
    { name: "Decoding", ms: 0.12, info: null },
    { name: "Audio playback", ms: 1.33, info: "64 samples @ 48000 Hz" },
  ],
  downstream_total_ms: 11.72,
  roundtrip_total_ms: 20.8,
};

const goodPeerAudio: PeerAudioInfo = {
  sample_rate: 48000,
  frame_size: 64,
  codec: "pcm",
  needs_resampling: false,
  channel_count: 2,
};

// Default (good connection)
export const Default: Story = {
  args: {
    network: goodNetworkStats,
    latency: goodLatency,
    underrunRate: 0,
    peerAudio: goodPeerAudio,
    localSampleRate: 48000,
  },
};

// Empty state (waiting for connection)
export const Empty: Story = {
  args: {
    network: null,
    latency: null,
  },
};

// High latency warning
export const HighLatency: Story = {
  args: {
    network: {
      ...goodNetworkStats,
      rtt_ms: 85.0,
      jitter_ms: 15.3,
    },
    latency: {
      upstream: [
        { name: "Audio capture", ms: 1.33, info: "64 samples @ 48000 Hz" },
        { name: "Encoding", ms: 0.15, info: null },
        { name: "Network (half RTT)", ms: 42.5, info: null },
      ],
      upstream_total_ms: 43.98,
      downstream: [
        { name: "Network (half RTT)", ms: 42.5, info: null },
        { name: "Jitter buffer", ms: 10.67, info: "512 samples @ 48000 Hz" },
        { name: "Decoding", ms: 0.12, info: null },
        { name: "Audio playback", ms: 1.33, info: "64 samples @ 48000 Hz" },
      ],
      downstream_total_ms: 54.62,
      roundtrip_total_ms: 98.6,
    },
    underrunRate: 0.2,
    peerAudio: goodPeerAudio,
    localSampleRate: 48000,
  },
};

// With underrun warnings
export const WithUnderruns: Story = {
  args: {
    network: goodNetworkStats,
    latency: goodLatency,
    underrunRate: 2.5,
    peerAudio: goodPeerAudio,
    localSampleRate: 48000,
  },
};

// Resampling active (peer has different sample rate)
export const ResamplingActive: Story = {
  args: {
    network: goodNetworkStats,
    latency: goodLatency,
    underrunRate: 0,
    peerAudio: {
      sample_rate: 44100,
      frame_size: 64,
      codec: "pcm",
      needs_resampling: true,
      channel_count: 2,
    },
    localSampleRate: 48000,
  },
};

// High quality (96kHz)
export const HighQuality: Story = {
  args: {
    network: goodNetworkStats,
    latency: {
      upstream: [
        { name: "Audio capture", ms: 0.67, info: "64 samples @ 96000 Hz" },
        { name: "Encoding", ms: 0.08, info: null },
        { name: "Network (half RTT)", ms: 7.6, info: null },
      ],
      upstream_total_ms: 8.35,
      downstream: [
        { name: "Network (half RTT)", ms: 7.6, info: null },
        { name: "Jitter buffer", ms: 1.33, info: "128 samples @ 96000 Hz" },
        { name: "Decoding", ms: 0.06, info: null },
        { name: "Audio playback", ms: 0.67, info: "64 samples @ 96000 Hz" },
      ],
      downstream_total_ms: 9.66,
      roundtrip_total_ms: 18.01,
    },
    underrunRate: 0,
    peerAudio: {
      sample_rate: 96000,
      frame_size: 64,
      codec: "pcm",
      needs_resampling: false,
      channel_count: 2,
    },
    localSampleRate: 96000,
  },
};

// Packet loss
export const PacketLoss: Story = {
  args: {
    network: {
      ...goodNetworkStats,
      packet_loss_percent: 5.2,
      quality: 'poor',
      measured_bps: 900_000,
      required_bps: 3_100_000,
      bandwidth_status: 'insufficient',
      packets_received: 51300,
      encryption: 'encrypted',
    },
    latency: goodLatency,
    underrunRate: 1.2,
    peerAudio: goodPeerAudio,
    localSampleRate: 48000,
  },
};

// Long session (large byte counts)
export const LongSession: Story = {
  args: {
    network: {
      ...goodNetworkStats,
      uptime_seconds: 7200,
      packets_sent: 432000,
      packets_received: 431800,
      encryption: 'encrypted',
      bytes_sent: 100663296,
      bytes_received: 99614720,
    },
    latency: goodLatency,
    underrunRate: 0.1,
    peerAudio: goodPeerAudio,
    localSampleRate: 48000,
  },
};

// Without peer audio info (legacy mode)
export const NoPeerAudio: Story = {
  args: {
    network: goodNetworkStats,
    latency: goodLatency,
    underrunRate: 0,
    peerAudio: null,
    localSampleRate: 48000,
  },
};

// Opus codec
export const OpusCodec: Story = {
  args: {
    network: goodNetworkStats,
    latency: {
      ...goodLatency,
      upstream: [
        { name: "Audio capture", ms: 1.33, info: "64 samples @ 48000 Hz" },
        { name: "Opus encoding", ms: 2.5, info: "128 kbps" },
        { name: "Network (half RTT)", ms: 7.6, info: null },
      ],
      upstream_total_ms: 11.43,
      downstream: [
        { name: "Network (half RTT)", ms: 7.6, info: null },
        { name: "Jitter buffer", ms: 2.67, info: "128 samples @ 48000 Hz" },
        { name: "Opus decoding", ms: 1.8, info: null },
        { name: "Audio playback", ms: 1.33, info: "64 samples @ 48000 Hz" },
      ],
      downstream_total_ms: 13.4,
      roundtrip_total_ms: 24.83,
    },
    underrunRate: 0,
    peerAudio: {
      sample_rate: 48000,
      frame_size: 960,
      codec: "opus",
      needs_resampling: false,
      channel_count: 2,
    },
    localSampleRate: 48000,
  },
};
