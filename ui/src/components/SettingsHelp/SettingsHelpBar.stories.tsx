import type { Meta, StoryObj } from "@storybook/react-vite";
import { SettingsHelpBar } from "./SettingsHelpBar";

const meta = {
  title: "Components/SettingsHelp/SettingsHelpBar",
  component: SettingsHelpBar,
  decorators: [
    (Story) => (
      <div style={{ width: "560px", padding: "16px", background: "var(--color-bg-primary)" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof SettingsHelpBar>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Being helped: who helps, and Stop */
export const Default: Story = {
  args: {
    message: "Aki is helping with your audio settings",
    actions: [{ label: "Stop", onClick: () => {}, testId: "stop" }],
  },
};

/** Offered help, waiting for the answer */
export const Asking: Story = {
  args: {
    message: "Waiting for Bo to answer",
    actions: [{ label: "Cancel", onClick: () => {}, testId: "cancel" }],
  },
};

/** Helping, with the settings panel closed */
export const Helping: Story = {
  args: {
    message: "Helping Bo with audio settings",
    status: "Bo declined the change",
    actions: [
      { label: "Open settings", onClick: () => {}, testId: "open" },
      { label: "Stop helping", onClick: () => {}, testId: "stop" },
    ],
  },
};

/** No actions (not used on screen; shows the text alone) */
export const Empty: Story = {
  args: {
    message: "Helping Bo with audio settings",
    actions: [],
  },
};
