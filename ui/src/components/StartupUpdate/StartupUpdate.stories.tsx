import type { Meta, StoryObj } from "@storybook/react-vite";
import { StartupUpdate } from "./StartupUpdate";

const meta = {
  title: "Screens/StartupUpdate",
  component: StartupUpdate,
  parameters: { layout: "fullscreen" },
  tags: ["autodocs"],
  args: { progress: { phase: "checking" }, onSkip: () => {} },
} satisfies Meta<typeof StartupUpdate>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: { progress: { phase: "downloading", received: 21_000_000, total: 60_000_000 } },
};

export const Checking: Story = {};

/** The server did not say how big the download is */
export const DownloadingSizeUnknown: Story = {
  args: { progress: { phase: "downloading", received: 21_000_000, total: null } },
};

export const Skipping: Story = {
  args: { progress: { phase: "downloading", received: 21_000_000, total: 60_000_000 }, skipping: true },
};

export const Installing: Story = { args: { progress: { phase: "installing" } } };

export const Restarting: Story = { args: { progress: { phase: "restarting" } } };
