import type { Meta, StoryObj } from "@storybook/react-vite";
import { InputLevelMeter } from "./InputLevelMeter";

const meta = {
  title: "Components/Mixer/InputLevelMeter",
  component: InputLevelMeter,
  parameters: {
    layout: "centered",
  },
  decorators: [
    (Story) => (
      <div style={{ width: "240px", background: "var(--color-bg-primary)", padding: "16px" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
  argTypes: {
    level: {
      control: { type: "range", min: 0, max: 100, step: 1 },
    },
  },
} satisfies Meta<typeof InputLevelMeter>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    level: 45,
  },
};

export const HighLevel: Story = {
  args: {
    level: 90,
  },
};

export const Empty: Story = {
  args: {
    level: 0,
  },
};

export const Muted: Story = {
  args: {
    level: 45,
    isMuted: true,
  },
};

export const Mini: Story = {
  args: {
    level: 45,
    variant: "mini",
  },
};
