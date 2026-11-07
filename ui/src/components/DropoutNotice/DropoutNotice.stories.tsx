import type { Meta, StoryObj } from "@storybook/react-vite";
import { DropoutNotice } from "./DropoutNotice";

const meta = {
  title: "Components/Room/DropoutNotice",
  component: DropoutNotice,
  parameters: { layout: "centered" },
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div style={{ padding: "20px", width: 240 }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof DropoutNotice>;

export default meta;
type Story = StoryObj<typeof meta>;

export const MyLine: Story = {
  args: { dropout: { silent_secs: 12, side: "mine" }, unstable: false, variant: "row" },
};

export const TheirLine: Story = {
  args: { dropout: { silent_secs: 7, side: "theirs" }, unstable: false, variant: "row" },
};

export const PathBetween: Story = {
  args: { dropout: { silent_secs: 31, side: "between" }, unstable: false, variant: "row" },
};

export const Checking: Story = {
  args: { dropout: { silent_secs: 2, side: "checking" }, unstable: false, variant: "row" },
};

export const NobodyCouldTell: Story = {
  args: { dropout: { silent_secs: 45, side: "unknown" }, unstable: false, variant: "row" },
};

export const UnstableOnly: Story = {
  args: { dropout: null, unstable: true, variant: "row" },
};

export const OnAMixerStrip: Story = {
  decorators: [
    (Story) => (
      <div style={{ width: 100 }}>
        <Story />
      </div>
    ),
  ],
  args: { dropout: { silent_secs: 12, side: "theirs" }, unstable: true, variant: "strip" },
};
