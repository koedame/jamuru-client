/**
 * Screens/HelpLog - the window the person being helped keeps beside the app to
 * see what the help did.
 */
import type { Meta, StoryObj } from "@storybook/react-vite";
import type { HelpRecord } from "../lib/tauri";
import { withBackend } from "../stories/fakeBackend";
import { HelpLogScreen } from "./HelpLogScreen";

const meta = {
  title: "Screens/HelpLog",
  component: HelpLogScreen,
  parameters: { layout: "centered" },
  decorators: [
    (Story) => (
      <div style={{ width: 360, background: "var(--color-bg-primary)" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof HelpLogScreen>;

export default meta;
type Story = StoryObj<typeof meta>;

const record = (changes: Partial<HelpRecord> = {}): HelpRecord => ({
  helper_name: "Aki",
  entries: [
    { at_ms: 1_000, kind: "started" },
    { at_ms: 9_000, kind: "setting_changed", setting: "buffer_size" },
    { at_ms: 15_000, kind: "muted" },
    { at_ms: 20_000, kind: "unmuted" },
  ],
  changed: ["buffer_size"],
  active: true,
  remaining_ms: 240_000,
  can_undo: true,
  ...changes,
});

export const Helping: Story = { decorators: [withBackend({ help_record_get: record() })] };

export const JustStarted: Story = {
  decorators: [
    withBackend({
      help_record_get: record({ entries: [{ at_ms: 1_000, kind: "started" }], changed: [], can_undo: false }),
    }),
  ],
};

export const Over: Story = {
  decorators: [withBackend({ help_record_get: record({ active: false, remaining_ms: null }) })],
};

export const NothingToShow: Story = { decorators: [withBackend({ help_record_get: null })] };
