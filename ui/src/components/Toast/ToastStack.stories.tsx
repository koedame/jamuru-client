import type { Meta, StoryObj } from "@storybook/react-vite";
import { Toast } from "./Toast";
import { ToastStack } from "./ToastStack";

const meta = {
  title: "Components/Common/ToastStack",
  component: ToastStack,
  parameters: { layout: "fullscreen" },
  decorators: [
    (Story) => (
      <div style={{ position: "relative", height: "240px", background: "var(--color-bg-primary)" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof ToastStack>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Empty: Story = { args: { children: null } };

export const Stacked: Story = {
  args: {
    children: (
      <>
        <Toast type="success" message="コピーしました" />
        <Toast type="warning" message="接続が不安定です" />
      </>
    ),
  },
};
