import { useEffect, useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { MixerPanel } from "./MixerPanel";

const meta = {
  title: "Windows/Mixer",
  component: MixerPanel,
  parameters: {
    layout: "fullscreen",
  },
  decorators: [
    (Story) => (
      <div style={{ height: "560px", display: "flex", background: "var(--color-bg-primary)" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof MixerPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Full mixer console with animated levels
 */
export const Console: Story = {
  args: {
    channels: [],
  },
  render: () => {
    const [channels, setChannels] = useState([
      {
        id: "local-1",
        name: "You",
        type: "local" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 0,
        levelR: 0,
        volume: 80,
        pan: 0,
        isMuted: false,
      },
      {
        id: "remote-1",
        name: "Alice",
        type: "remote" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 0,
        levelR: 0,
        volume: 75,
        pan: -30,
        isMuted: false,
      },
      {
        id: "remote-2",
        name: "Bob",
        type: "remote" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 0,
        levelR: 0,
        volume: 70,
        pan: 30,
        isMuted: false,
      },
    ]);

    // Animate levels
    useEffect(() => {
      const interval = setInterval(() => {
        setChannels((prev) =>
          prev.map((ch) => ({
            ...ch,
            levelL: ch.isMuted ? 0 : 30 + Math.random() * 50,
            levelR: ch.isMuted ? 0 : 30 + Math.random() * 50,
          }))
        );
      }, 100);
      return () => clearInterval(interval);
    }, []);

    return (
      <MixerPanel
        channels={channels}
        onChannelVolumeChange={(id, volume) => {
          setChannels((prev) =>
            prev.map((ch) => (ch.id === id ? { ...ch, volume } : ch))
          );
        }}
        onChannelPanChange={(id, pan) => {
          setChannels((prev) =>
            prev.map((ch) => (ch.id === id ? { ...ch, pan } : ch))
          );
        }}
        onChannelMuteToggle={(id) => {
          setChannels((prev) =>
            prev.map((ch) =>
              ch.id === id ? { ...ch, isMuted: !ch.isMuted } : ch
            )
          );
        }}
      />
    );
  },
};

/**
 * Static view with default values
 */
export const Default: Story = {
  args: {
    channels: [
      {
        id: "local-1",
        name: "You",
        type: "local" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 65,
        levelR: 60,
        volume: 80,
        pan: 0,
        isMuted: false,
      },
      {
        id: "remote-1",
        name: "Alice",
        type: "remote" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 55,
        levelR: 50,
        volume: 75,
        pan: -30,
        isMuted: false,
      },
    ],
  },
};

/** One participant's audio stopped without a leave notice: their strip says so and whose line is cut */
export const AudioStopped: Story = {
  args: {
    channels: [
      {
        id: "local-1",
        name: "You",
        type: "local" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 65,
        levelR: 60,
        volume: 80,
        pan: 0,
        isMuted: false,
      },
      {
        id: "remote-1",
        name: "Alice",
        type: "remote" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 0,
        levelR: 0,
        volume: 75,
        pan: -30,
        isMuted: false,
        dropout: { silent_secs: 12, side: "theirs" as const },
        unstable: true,
      },
      {
        id: "remote-2",
        name: "Bob",
        type: "remote" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 50,
        levelR: 48,
        volume: 80,
        pan: 30,
        isMuted: false,
      },
    ],
  },
};

/** No channel yet: only the title (a session with no audio link) */
export const Empty: Story = {
  args: { channels: [] },
};

/**
 * Single user (no remote participants). Named for that, not the removed
 * per-channel solo/isolate button.
 */
export const SingleParticipant: Story = {
  args: {
    channels: [
      {
        id: "local-1",
        name: "You",
        type: "local" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 70,
        levelR: 65,
        volume: 80,
        pan: 0,
        isMuted: false,
      },
    ],
  },
};

/**
 * Many participants
 */
export const Crowded: Story = {
  args: {
    channels: [
      {
        id: "local-1",
        name: "You",
        type: "local" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 70,
        levelR: 65,
        volume: 80,
        pan: 0,
        isMuted: false,
      },
      {
        id: "remote-1",
        name: "Alice",
        type: "remote" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 60,
        levelR: 55,
        volume: 75,
        pan: -50,
        isMuted: false,
      },
      {
        id: "remote-2",
        name: "Bob",
        type: "remote" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 50,
        levelR: 45,
        volume: 70,
        pan: 50,
        isMuted: false,
      },
      {
        id: "remote-3",
        name: "Charlie",
        type: "remote" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 55,
        levelR: 55,
        volume: 85,
        pan: -25,
        isMuted: false,
      },
      {
        id: "remote-4",
        name: "Diana",
        type: "remote" as const,
        sampleRate: 48000,
        channelCount: 2,
        levelL: 65,
        levelR: 60,
        volume: 78,
        pan: 25,
        isMuted: true,
      },
    ],
  },
};
