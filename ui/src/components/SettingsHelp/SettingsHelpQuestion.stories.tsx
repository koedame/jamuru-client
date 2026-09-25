import type { Meta, StoryObj } from "@storybook/react-vite";
import { SettingsHelpQuestion } from "./SettingsHelpQuestion";

const meta = {
  title: "Components/SettingsHelp/SettingsHelpQuestion",
  component: SettingsHelpQuestion,
  parameters: {
    layout: "fullscreen",
  },
  decorators: [
    (Story) => (
      <div style={{ position: "relative", width: "600px", height: "400px", background: "var(--color-bg-primary)" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
  argTypes: {
    open: { control: "boolean" },
  },
} satisfies Meta<typeof SettingsHelpQuestion>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Someone asks to help with the settings */
export const Default: Story = {
  args: {
    open: true,
    message: "Aki wants to help with your audio settings",
    allowLabel: "Allow",
    declineLabel: "Decline",
    onAllow: () => {},
    onDecline: () => {},
  },
};

/** The helper proposes one change */
export const Proposal: Story = {
  args: {
    open: true,
    message: "Aki wants to change your Input Device to Scarlett 2i2 USB",
    allowLabel: "Allow change",
    declineLabel: "Decline",
    onAllow: () => {},
    onDecline: () => {},
  },
};

/** A long device name wraps inside the card */
export const LongValue: Story = {
  args: {
    open: true,
    message:
      "Aki wants to change your Output Device to Focusrite Scarlett 18i20 3rd Gen (Line Outputs 3-4, Speakers B)",
    allowLabel: "Allow change",
    declineLabel: "Decline",
    onAllow: () => {},
    onDecline: () => {},
  },
};

/** Nothing is asked */
export const Closed: Story = {
  args: {
    open: false,
    message: "",
    allowLabel: "Allow",
    declineLabel: "Decline",
    onAllow: () => {},
    onDecline: () => {},
  },
};
