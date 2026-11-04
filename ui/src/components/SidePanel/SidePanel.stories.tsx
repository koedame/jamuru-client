import type { Meta, StoryObj } from "@storybook/react-vite";
import { SidePanel } from "./SidePanel";

const meta = {
  title: "Components/Common/SidePanel",
  component: SidePanel,
  parameters: {
    layout: "fullscreen",
  },
  decorators: [
    (Story) => (
      <div style={{ position: "relative", width: "100%", height: "500px", background: "var(--color-bg-primary)" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof SidePanel>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Open: Story = {
  args: {
    isOpen: true,
    onClose: () => {},
    title: "設定",
    children: <p style={{ color: "var(--color-text-primary)" }}>パネルの内容がここに表示されます。</p>,
  },
};

export const Closed: Story = {
  args: {
    isOpen: false,
    onClose: () => {},
    title: "設定",
    children: <p>非表示状態</p>,
  },
};

export const Empty: Story = {
  args: {
    isOpen: true,
    onClose: () => {},
    title: "空の状態",
    children: null,
  },
};
