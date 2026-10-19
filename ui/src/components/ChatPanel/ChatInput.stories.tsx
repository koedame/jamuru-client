import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { ChatInput } from "./ChatInput";

const meta = {
  title: "Components/Chat/ChatInput",
  component: ChatInput,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div
        style={{
          width: "300px",
          padding: "16px",
          background: "var(--color-bg-secondary)",
        }}
      >
        <Story />
      </div>
    ),
  ],
  args: {
    onSend: fn(),
  },
} satisfies Meta<typeof ChatInput>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    placeholder: "メッセージを入力...",
  },
};

export const CustomPlaceholder: Story = {
  args: {
    placeholder: "Type a message...",
  },
};

export const Disabled: Story = {
  args: {
    placeholder: "接続されていません",
    disabled: true,
  },
};

export const CustomSendLabel: Story = {
  args: {
    placeholder: "メッセージを入力...",
    sendLabel: "Send",
  },
};

export const MaxRows2: Story = {
  args: {
    placeholder: "最大2行まで表示",
    maxRows: 2,
  },
};

export const MaxRows6: Story = {
  args: {
    placeholder: "最大6行まで表示",
    maxRows: 6,
  },
};

/** Send a message to see the failure: the text stays and an error shows. */
export const SendFails: Story = {
  args: {
    onSend: () => Promise.reject(new Error("offline")),
  },
};
