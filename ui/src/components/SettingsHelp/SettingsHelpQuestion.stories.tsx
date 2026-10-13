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

/** Someone asks for help with their settings: the question the helper answers */
export const Default: Story = {
  args: {
    open: true,
    message: "Aki has asked for help with settings",
    detail: "You'll look at Aki's jamuru settings together.",
    allowLabel: "Help",
    declineLabel: "Not now",
    onAllow: () => {},
    onDecline: () => {},
  },
};

/** The same, in Japanese: the two buttons fit the card */
export const Japanese: Story = {
  args: {
    open: true,
    message: "あきさんから設定依頼が届きました",
    detail: "あきさんの jamuru の設定を、一緒に見て手伝います。",
    allowLabel: "手伝う",
    declineLabel: "今はできない",
    onAllow: () => {},
    onDecline: () => {},
  },
};

/** A long name wraps inside the card */
export const LongName: Story = {
  args: {
    open: true,
    message: "Aki Takahashi-Montgomery the Third of Osaka has asked for help with settings",
    allowLabel: "Help",
    declineLabel: "Not now",
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
