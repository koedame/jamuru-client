import type { Meta, StoryObj } from "@storybook/react-vite";
import { AboutTab } from "./AboutTab";

const meta = {
  title: "Components/Settings/Tabs/AboutTab",
  component: AboutTab,
  parameters: { layout: "centered" },
  decorators: [
    (Story) => (
      <div style={{ width: "450px", background: "var(--color-bg-primary)", padding: "16px" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
  args: {
    onOpenTerms: () => {},
    onOpenLicense: () => {},
    onOpenPrivacy: () => {},
    onOpenAnnouncements: () => {},
  },
} satisfies Meta<typeof AboutTab>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const WithUpdateCheck: Story = {
  args: {
    update: { current: "0.9.0", support: "install", phase: "available", version: "0.10.0", onCheck: () => {}, onInstall: () => {} },
  },
};
