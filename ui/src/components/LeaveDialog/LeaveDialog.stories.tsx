import type { Meta, StoryObj } from "@storybook/react-vite";
import { LeaveDialog } from "./LeaveDialog";

const meta = {
  title: "Components/Room/LeaveDialog",
  component: LeaveDialog,
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
    pending: { control: "boolean" },
  },
} satisfies Meta<typeof LeaveDialog>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    open: true,
    pending: false,
    onConfirm: () => {},
    onCancel: () => {},
  },
};

export const Loading: Story = {
  args: {
    open: true,
    pending: true,
    onConfirm: () => {},
    onCancel: () => {},
  },
};

export const Closed: Story = {
  args: {
    open: false,
    onConfirm: () => {},
    onCancel: () => {},
  },
};
