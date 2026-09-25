import type { Meta, StoryObj } from "@storybook/react-vite";
import { SettingsHelpPanel } from "./SettingsHelpPanel";
import type { AudioSettingsTabProps } from "../SettingsPanel/useAudioSettingsTab";

const devicesTab: AudioSettingsTabProps = {
  inputDevices: [
    { id: "device-1", name: "Scarlett 2i2 USB" },
    { id: "device-2", name: "MacBook Pro Microphone", isDefault: true },
  ],
  outputDevices: [
    { id: "device-1", name: "Scarlett 2i2 USB" },
    { id: "device-3", name: "MacBook Pro Speakers", isDefault: true },
  ],
  selectedInputId: "device-2",
  selectedOutputId: "device-3",
  inputChannelOptions: [
    { value: "1", label: "Ch 1" },
    { value: "2", label: "Ch 2" },
  ],
  outputChannelOptions: [
    { value: "1", label: "Ch 1" },
    { value: "2", label: "Ch 2" },
  ],
  selectedInputChannelL: "1",
  selectedInputChannelR: "2",
  selectedOutputChannelL: "1",
  selectedOutputChannelR: "2",
  sampleRateOptions: [
    { value: "44100", label: "44100 Hz" },
    { value: "48000", label: "48000 Hz (Recommended)" },
  ],
  selectedSampleRate: "48000",
  bufferSizeOptions: [
    { value: "64", label: "64 samples (1.33ms)" },
    { value: "128", label: "128 samples (2.67ms)" },
  ],
  selectedBufferSize: "64",
  transmitChannelOptions: [
    { value: "1", label: "Mono" },
    { value: "2", label: "Stereo" },
  ],
  selectedTransmitChannels: "2",
  onInputDeviceChange: () => {},
  onOutputDeviceChange: () => {},
  onInputChannelLChange: () => {},
  onInputChannelRChange: () => {},
  onOutputChannelLChange: () => {},
  onOutputChannelRChange: () => {},
  onSampleRateChange: () => {},
  onBufferSizeChange: () => {},
  onTransmitChannelsChange: () => {},
};

const meta = {
  title: "Components/SettingsHelp/SettingsHelpPanel",
  component: SettingsHelpPanel,
  decorators: [
    (Story) => (
      <div style={{ width: "420px", padding: "16px", background: "var(--color-bg-secondary)" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof SettingsHelpPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The other person's settings, ready to change */
export const Default: Story = {
  args: { devicesTab },
};

/** A change is on its way to the other person's app; further choices wait */
export const Waiting: Story = {
  args: { devicesTab, waiting: true },
};

/** The other person's app could not apply the last change */
export const Refused: Story = {
  args: { devicesTab, status: "Bo's app could not switch: that device is no longer connected" },
};

/** The other person's app lists no devices */
export const Empty: Story = {
  args: {
    devicesTab: { ...devicesTab, inputDevices: [], outputDevices: [], selectedInputId: null, selectedOutputId: null },
  },
};
