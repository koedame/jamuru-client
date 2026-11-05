/**
 * Screens/StartupUpdateScreen - the small window a start shows while it updates
 * the app before opening it, reading how far the update is from the backend.
 * (The window's look in each phase is in `Screens/StartupUpdate`.)
 */
import type { Meta, StoryObj } from "@storybook/react-vite";
import { withBackend } from "../stories/fakeBackend";
import { StartupUpdateScreen } from "./StartupUpdateScreen";

const meta = {
  title: "Screens/StartupUpdateScreen",
  component: StartupUpdateScreen,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof StartupUpdateScreen>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Checking: Story = {
  decorators: [withBackend({ update_startup_progress: { phase: "checking" } })],
};

export const Downloading: Story = {
  decorators: [
    withBackend({ update_startup_progress: { phase: "downloading", received: 12_000_000, total: 30_000_000 } }),
  ],
};
