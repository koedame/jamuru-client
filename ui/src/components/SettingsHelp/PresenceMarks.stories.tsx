import type { Meta, StoryObj } from "@storybook/react-vite";
import { PresenceMarks } from "./PresenceMarks";

const meta = {
  title: "Components/SettingsHelp/PresenceMarks",
  component: PresenceMarks,
  decorators: [
    (Story) => (
      <div style={{ width: "560px", height: "160px", padding: "16px", background: "var(--color-bg-primary)" }}>
        <button style={{ position: "fixed", left: 40, top: 60, width: 160, height: 40 }}>Mute</button>
        <input style={{ position: "fixed", left: 260, top: 60, width: 200, height: 40 }} defaultValue="Aki" />
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof PresenceMarks>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The part under the other person's pointer, and the place in it */
export const Hover: Story = {
  args: {
    name: "Aki",
    marks: [{ kind: "hover", left: 40, top: 60, width: 160, height: 40, at: { x: 0.7, y: 0.4 } }],
  },
};

/** The part the other person has focused, next to the one their pointer is over */
export const HoverAndFocus: Story = {
  args: {
    name: "Aki",
    marks: [
      { kind: "hover", left: 40, top: 60, width: 160, height: 40, at: { x: 0.2, y: 0.5 } },
      { kind: "focus", left: 260, top: 60, width: 200, height: 40 },
    ],
  },
};
