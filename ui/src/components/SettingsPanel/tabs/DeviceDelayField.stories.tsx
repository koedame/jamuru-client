import type { Meta, StoryObj } from "@storybook/react-vite";
import type { DeviceDelayStatus } from "../../../lib/tauri";
import { DeviceDelayField } from "./DeviceDelayField";

const meta = {
  title: "Components/Settings/DeviceDelayField",
  component: DeviceDelayField,
  parameters: { layout: "centered" },
  decorators: [
    (Story) => (
      <div style={{ width: "450px", background: "var(--color-bg-primary)", padding: "16px" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
  args: { canStart: true, onStart: () => {}, onCancel: () => {} },
} satisfies Meta<typeof DeviceDelayField>;

export default meta;
type Story = StoryObj<typeof meta>;

const idle: DeviceDelayStatus = {
  session_running: true,
  running: false,
  stimulus: null,
  step: null,
  steps: 6,
  failure: null,
  figure: null,
  peers: {},
};

export const Default: Story = { args: { status: idle } };

export const Loading: Story = { args: { status: null } };

export const NeedsRoom: Story = { args: { status: { ...idle, session_running: false } } };

export const Running: Story = {
  args: { status: { ...idle, running: true, stimulus: "clicks", step: 2 } },
};

export const Measured: Story = {
  args: {
    status: {
      ...idle,
      figure: { delay_ms: 11.4, spread_ms: 0.3, stability: "steady", needs_review: false, measured_at: 1767225600 },
    },
  },
};

export const MeasuredShaky: Story = {
  args: {
    status: {
      ...idle,
      figure: { delay_ms: 38.2, spread_ms: 4.1, stability: "shaky", needs_review: true, measured_at: 1767225600 },
    },
  },
};

export const Failed: Story = { args: { status: { ...idle, failure: "no_sound" } } };

export const StartRefused: Story = { args: { status: idle, startFailed: true } };

export const HelperCannotStart: Story = { args: { status: idle, canStart: false } };
