import type { Meta, StoryObj } from "@storybook/react-vite";
import { MuteButton } from "./MuteButton";

const meta = {
  title: "Components/Mixer/MuteButton",
  component: MuteButton,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
} satisfies Meta<typeof MuteButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Unmuted: Story = {
  args: {
    isMuted: false,
    onToggle: () => {},
  },
};

export const Muted: Story = {
  args: {
    isMuted: true,
    onToggle: () => {},
  },
};

export const Disabled: Story = {
  args: {
    isMuted: false,
    onToggle: () => {},
    disabled: true,
  },
};
