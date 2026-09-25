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

/** Someone asks to help with the settings: the one question the helped side answers */
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

/** The same, in Japanese: the two buttons fit the card */
export const Japanese: Story = {
  args: {
    open: true,
    message: "Akiさんが音声の設定を手伝いたいそうです",
    allowLabel: "許可する",
    declineLabel: "断る",
    onAllow: () => {},
    onDecline: () => {},
  },
};

/** A long name wraps inside the card */
export const LongName: Story = {
  args: {
    open: true,
    message: "Aki Takahashi-Montgomery the Third of Osaka wants to help with your audio settings",
    allowLabel: "Allow",
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
