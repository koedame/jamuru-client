import type { Meta, StoryObj } from "@storybook/react-vite";
import { RecordingMark, RecordingPanel } from "./RecordingPanel";

const meta = {
  title: "Components/RecordingPanel",
  component: RecordingPanel,
  parameters: { layout: "centered" },
  decorators: [
    (Story) => (
      <div style={{ width: "208px", background: "var(--color-bg-secondary)", padding: "16px" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
  args: {
    recording: false,
    seconds: 0,
    fileName: null,
    withSelf: false,
    last: null,
    onWithSelfChange: () => {},
    onStart: () => {},
    onStop: () => {},
    onOpenFolder: () => {},
  },
} satisfies Meta<typeof RecordingPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

const ended = {
  id: 1,
  file_name: "jamuru-2026-11-04-153012.wav",
  self_file_name: null,
  seconds: 75,
  dropped_frames: 0,
  ended_by: "stopped",
  error: null,
} as const;

export const Idle: Story = {};

export const IdleWithMicrophone: Story = { args: { withSelf: true } };

export const Recording: Story = {
  args: { recording: true, seconds: 3725, fileName: "jamuru-2026-11-04-153012.wav" },
};

export const Saved: Story = { args: { last: ended } };

export const SavedWithGap: Story = { args: { last: { ...ended, dropped_frames: 4800 } } };

export const DiskFull: Story = {
  args: { last: { ...ended, ended_by: "write_failed", error: "No space left on device" } },
};

export const CouldNotStart: Story = {
  args: { error: "The recording file could not be made in /home/me/Music/jamuru" },
};

export const ParticipantMark: StoryObj<typeof RecordingMark> = {
  render: () => <RecordingMark />,
};
