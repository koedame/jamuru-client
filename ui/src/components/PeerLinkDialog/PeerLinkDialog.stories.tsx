import type { Meta, StoryObj } from "@storybook/react-vite";
import { PeerLinkDialog } from "./PeerLinkDialog";
import type { PeerLinkStatus } from "../../lib/tauri";
import type { BufferRequests } from "../BufferRequest";

/** No ask made or received */
const idle: BufferRequests = {
  askedOf: () => null,
  heldFor: () => null,
  canAsk: true,
  ask: async () => {},
  withdraw: async () => {},
  restore: async () => {},
  toast: null,
  overlays: null,
};

const meta = {
  title: "Components/Room/PeerLinkDialog",
  component: PeerLinkDialog,
  parameters: {
    layout: "fullscreen",
  },
  tags: ["autodocs"],
  args: { onClose: () => {}, peerId: "b", askable: true, requests: idle },
} satisfies Meta<typeof PeerLinkDialog>;

export default meta;
type Story = StoryObj<typeof meta>;

const upLink: PeerLinkStatus = {
  peer_id: "b",
  connection_state: "connected",
  connection_error: null,
  network: {
    rtt_ms: 12.4,
    jitter_ms: 0.8,
    packet_loss_percent: 0.5,
    quality: "good",
    measured_bps: 0,
    required_bps: 0,
    bandwidth_status: "sufficient",
    uptime_seconds: 60,
    packets_sent: 1000,
    packets_received: 1000,
    encryption: "encrypted",
    bytes_sent: 0,
    bytes_received: 0,
  },
  latency: {
    upstream: [
      { name: "Capture buffer", ms: 0.67, info: null },
      { name: "Network", ms: 6.2, info: null },
    ],
    upstream_total_ms: 6.87,
    downstream: [
      { name: "Network", ms: 6.2, info: null },
      { name: "Jitter buffer", ms: 2, info: null },
      { name: "Playback buffer", ms: 1, info: null },
    ],
    downstream_total_ms: 9.2,
    roundtrip_total_ms: 16.07,
  },
  peer_audio: {
    sample_rate: 48000,
    frame_size: 64,
    codec: "pcm",
    needs_resampling: false,
    channel_count: 2,
    capture_buffer_ms: 1.33,
    playback_buffer_ms: 2.67,
    jitter_buffer_ms: 5,
  },
  level: 0,
  buffer: { own: { mode: "preset" }, delay_ms10: 0, granted: null },
};

export const Default: Story = {
  args: { peerName: "Aki", link: upLink, localSampleRate: 48000 },
};

/** The link is not up yet: every value waits for its measurement */
export const Empty: Story = {
  args: { peerName: "Aki", link: undefined, localSampleRate: 48000 },
};

export const LongName: Story = {
  args: { peerName: "とても長い名前の参加者のひとり", link: upLink, localSampleRate: 48000 },
};

/** The other app does not take asks: only the user's own buffer is offered */
export const CannotAsk: Story = {
  args: { peerName: "Aki", link: upLink, localSampleRate: 48000, askable: false },
};

/** The user holds the buffer at a length of their own */
export const Held: Story = {
  args: {
    peerName: "Aki",
    link: { ...upLink, buffer: { own: { mode: "fixed", ms10: 150 }, delay_ms10: 150, granted: null } },
    localSampleRate: 48000,
  },
};
