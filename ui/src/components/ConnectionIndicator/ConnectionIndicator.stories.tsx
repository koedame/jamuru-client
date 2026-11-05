import type { Meta, StoryObj } from "@storybook/react-vite";
import { ConnectionIndicator } from "./ConnectionIndicator";

const meta = {
  title: "Components/Room/ConnectionIndicator",
  component: ConnectionIndicator,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div style={{ padding: "20px" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof ConnectionIndicator>;

export default meta;
type Story = StoryObj<typeof meta>;

// Basic status states
export const Disconnected: Story = {
  args: {
    status: "disconnected",
    showLatency: false,
  },
};

export const Connecting: Story = {
  args: {
    status: "connecting",
    showLatency: false,
  },
};

export const Connected: Story = {
  args: {
    status: "connected",
    upstreamLatencyMs: 12.5,
    downstreamLatencyMs: 13.2,
    showLatency: true,
  },
};

export const Unstable: Story = {
  args: {
    status: "unstable",
    upstreamLatencyMs: 85.3,
    downstreamLatencyMs: 92.1,
    showLatency: true,
  },
};

export const Error: Story = {
  args: {
    status: "error",
    showLatency: false,
  },
};

// With latency display
export const ConnectedWithLatency: Story = {
  args: {
    status: "connected",
    upstreamLatencyMs: 8.2,
    downstreamLatencyMs: 9.1,
    showLatency: true,
  },
};

export const ConnectedWithLegacyLatency: Story = {
  name: "Connected (Legacy RTT)",
  args: {
    status: "connected",
    latencyMs: 15.5,
    showLatency: true,
  },
};

export const HighLatency: Story = {
  args: {
    status: "unstable",
    upstreamLatencyMs: 150.0,
    downstreamLatencyMs: 165.3,
    showLatency: true,
  },
};

// Size variants
export const SizeSmall: Story = {
  args: {
    status: "connected",
    upstreamLatencyMs: 10.5,
    downstreamLatencyMs: 11.2,
    showLatency: true,
    size: "sm",
  },
};

export const SizeMedium: Story = {
  args: {
    status: "connected",
    upstreamLatencyMs: 10.5,
    downstreamLatencyMs: 11.2,
    showLatency: true,
    size: "md",
  },
};

export const SizeLarge: Story = {
  args: {
    status: "connected",
    upstreamLatencyMs: 10.5,
    downstreamLatencyMs: 11.2,
    showLatency: true,
    size: "lg",
  },
};

// Clickable
export const Clickable: Story = {
  args: {
    status: "connected",
    upstreamLatencyMs: 12.0,
    downstreamLatencyMs: 13.5,
    showLatency: true,
    onClick: () => alert("Connection indicator clicked!"),
  },
};

// All states overview
export const AllStates: Story = {
  args: {
    status: "connected",
    showLatency: true,
  },
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <ConnectionIndicator status="disconnected" showLatency={false} />
      <ConnectionIndicator status="connecting" showLatency={false} />
      <ConnectionIndicator
        status="connected"
        upstreamLatencyMs={12.5}
        downstreamLatencyMs={13.2}
        showLatency={true}
      />
      <ConnectionIndicator
        status="unstable"
        upstreamLatencyMs={85.0}
        downstreamLatencyMs={90.0}
        showLatency={true}
      />
      <ConnectionIndicator status="error" showLatency={false} />
    </div>
  ),
};

// All sizes overview
export const AllSizes: Story = {
  args: {
    status: "connected",
    showLatency: true,
  },
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <div>
        <span style={{ marginRight: "8px", color: "#888" }}>sm:</span>
        <ConnectionIndicator
          status="connected"
          upstreamLatencyMs={10.0}
          downstreamLatencyMs={10.5}
          showLatency={true}
          size="sm"
        />
      </div>
      <div>
        <span style={{ marginRight: "8px", color: "#888" }}>md:</span>
        <ConnectionIndicator
          status="connected"
          upstreamLatencyMs={10.0}
          downstreamLatencyMs={10.5}
          showLatency={true}
          size="md"
        />
      </div>
      <div>
        <span style={{ marginRight: "8px", color: "#888" }}>lg:</span>
        <ConnectionIndicator
          status="connected"
          upstreamLatencyMs={10.0}
          downstreamLatencyMs={10.5}
          showLatency={true}
          size="lg"
        />
      </div>
    </div>
  ),
};

// Without latency
export const NoLatencyDisplay: Story = {
  args: {
    status: "connected",
    upstreamLatencyMs: 10.0,
    downstreamLatencyMs: 10.5,
    showLatency: false,
  },
};

/**
 * Quality bands (REQ-LAT-121). The band is classified by the core library from
 * RTT and packet loss; the component only picks the colour.
 */
export const QualityGood: Story = {
  args: {
    status: 'connected',
    quality: 'good',
    latencyMs: 12,
  },
};

export const QualityFair: Story = {
  args: {
    status: 'connected',
    quality: 'fair',
    latencyMs: 55,
  },
};

export const QualityPoor: Story = {
  args: {
    status: 'unstable',
    quality: 'poor',
    latencyMs: 140,
  },
};

/** No measurement yet: the status colour is used and no quality class is set. */
export const QualityUnknown: Story = {
  args: {
    status: 'connecting',
  },
};

/** Audio device input/output latency, shown separately (REQ-LAT-122). */
export const WithDeviceLatency: Story = {
  args: {
    status: 'connected',
    quality: 'good',
    latencyMs: 18,
    inputLatencyMs: 3,
    outputLatencyMs: 3,
  },
};
