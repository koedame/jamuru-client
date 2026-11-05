import type { Meta, StoryObj } from "@storybook/react-vite";
import { PeerBufferSection } from "./PeerBufferSection";

const meta = {
  title: "Components/Room/PeerBufferSection",
  component: PeerBufferSection,
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div style={{ width: 328 }}>
        <Story />
      </div>
    ),
  ],
  args: {
    peerName: "Aki",
    buffer: { own: { mode: "preset" }, delay_ms10: 53, granted: null },
    held: null,
    asked: null,
    canAsk: true,
    showHint: false,
    onDismissHint: () => {},
    onSetOwn: () => {},
    onRestoreHeld: () => {},
    onAsk: () => {},
    onWithdraw: () => {},
  },
} satisfies Meta<typeof PeerBufferSection>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

/** The first time the controls can be used */
export const WithHint: Story = { args: { showHint: true } };

export const HeldByTheUser: Story = {
  args: { buffer: { own: { mode: "fixed", ms10: 150 }, delay_ms10: 150, granted: null } },
};

const terms = { max_ms10: 300, tolerance_tenths: 5, auto: true };

export const AskWaiting: Story = {
  args: { asked: { terms, standing: { state: "pending" } } },
};

export const AskAccepted: Story = {
  args: {
    asked: {
      terms,
      standing: {
        state: "granted",
        applied: { applied_ms10: 120, rounded: true, at_lowest: false, auto_running: true },
      },
    },
  },
};

export const AskDeclined: Story = {
  args: { asked: { terms, standing: { state: "declined" } } },
};

/** The user took up an ask: the buffer is longer than the preset's, and can be put back */
export const HoldingAnAsk: Story = {
  args: {
    buffer: {
      own: { mode: "preset" },
      delay_ms10: 120,
      granted: { applied_ms10: 120, rounded: false, at_lowest: false, auto_running: true },
    },
    held: { terms },
    canAsk: false,
  },
};
