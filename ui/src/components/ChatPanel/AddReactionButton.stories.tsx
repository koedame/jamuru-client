import type { Meta, StoryObj } from "@storybook/react-vite";
import { AddReactionButton } from "./AddReactionButton";

const meta = {
  title: "Components/Chat/AddReactionButton",
  component: AddReactionButton,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
} satisfies Meta<typeof AddReactionButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    onSelect: () => {},
  },
};

export const WithRecentEmojis: Story = {
  args: {
    onSelect: () => {},
    recentEmojis: ["👍", "❤️", "🎵"],
  },
};

export const Disabled: Story = {
  args: {
    onSelect: () => {},
    disabled: true,
  },
};
