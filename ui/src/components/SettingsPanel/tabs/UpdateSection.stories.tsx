import type { Meta, StoryObj } from "@storybook/react-vite";
import { UpdateSection } from "./UpdateSection";

const meta = {
  title: "Components/Settings/UpdateSection",
  component: UpdateSection,
  parameters: { layout: "centered" },
  decorators: [
    (Story) => (
      <div style={{ width: "450px", background: "var(--color-bg-primary)", padding: "16px" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
  args: { current: "0.9.0", support: "install", phase: "idle", onCheck: () => {}, onInstall: () => {} },
} satisfies Meta<typeof UpdateSection>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Idle: Story = {};

export const Loading: Story = { args: { current: null, support: null } };

export const Checking: Story = { args: { phase: "checking" } };

export const UpToDate: Story = { args: { phase: "up_to_date" } };

export const Available: Story = { args: { phase: "available", version: "0.10.0" } };

export const AvailableInSession: Story = { args: { phase: "available", version: "0.10.0", inSession: true } };

export const Installing: Story = { args: { phase: "installing", version: "0.10.0" } };

export const Busy: Story = { args: { phase: "busy", version: "0.10.0" } };

export const Error: Story = { args: { phase: "error", version: "0.10.0", error: "network unreachable" } };

export const NoticeOnly: Story = { args: { support: "notice", phase: "available", version: "0.10.0" } };

export const Unsupported: Story = { args: { support: "none" } };
