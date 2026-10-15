import type { Meta, StoryObj } from "@storybook/react-vite";
import { SettingsHelpBar } from "./SettingsHelpBar";
import { SettingsHelpLog } from "./SettingsHelpLog";

const meta = {
  title: "Components/SettingsHelp/SettingsHelpLog",
  component: SettingsHelpLog,
  decorators: [
    (Story) => (
      <div style={{ width: "360px", padding: "16px", background: "var(--color-bg-primary)" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof SettingsHelpLog>;

export default meta;
type Story = StoryObj<typeof meta>;

const lines = [
  { id: "1", time: "14:02:11", text: "Aki started helping" },
  { id: "2", time: "14:03:40", text: "Aki changed the Input Device setting" },
  { id: "3", time: "14:04:05", text: "Aki unmuted the microphone" },
];

/** The help goes on: who, how long until it ends by itself, Stop */
export const Helping: Story = {
  args: {
    heading: "Aki is helping",
    remaining: "If nothing is done, the help ends by itself in 27:12",
    lines,
    emptyText: "Nothing has been done yet",
    stopLabel: "Stop the help",
    onStop: () => {},
  },
};

/** Nothing has been done yet */
export const Empty: Story = {
  args: {
    heading: "Aki is helping",
    remaining: "If nothing is done, the help ends by itself in 30:00",
    lines: [],
    emptyText: "Nothing has been done yet",
    stopLabel: "Stop the help",
    onStop: () => {},
  },
};

/** The help is over: what it changed, with one press to put it back */
export const Over: Story = {
  args: {
    heading: "The help from Aki is over",
    lines,
    emptyText: "Nothing has been done yet",
    after: (
      <SettingsHelpBar
        message="Aki changed: Input Device, Buffer Size"
        actions={[{ label: "Put back", onClick: () => {}, testId: "undo" }]}
      />
    ),
  },
};
