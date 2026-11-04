import type { Meta, StoryObj } from "@storybook/react-vite";
import { Toast } from "./Toast";

const meta = {
  title: "Components/Common/Toast",
  component: Toast,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
  argTypes: {
    type: {
      control: "select",
      options: ["success", "error", "info", "warning"],
    },
  },
} satisfies Meta<typeof Toast>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Success: Story = {
  args: { type: "success", message: "コピーしました" },
};

export const Error: Story = {
  args: { type: "error", message: "エラーが発生しました" },
};

export const Info: Story = {
  args: { type: "info", message: "接続中..." },
};

export const Warning: Story = {
  args: { type: "warning", message: "接続が不安定です" },
};
