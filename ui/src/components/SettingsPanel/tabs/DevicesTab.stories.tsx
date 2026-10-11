import type { Meta, StoryObj } from "@storybook/react-vite";
import { DevicesTab } from "./DevicesTab";

const meta = {
  title: "Components/Settings/Tabs/DevicesTab",
  component: DevicesTab,
  parameters: {
    layout: "centered",
  },
  decorators: [
    (Story) => (
      <div style={{ width: "450px", background: "var(--color-bg-primary)", padding: "16px" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof DevicesTab>;

export default meta;
type Story = StoryObj<typeof meta>;

const inputDevices = [
  { id: "default-in", name: "MacBook Pro Microphone", isDefault: true },
  { id: "usb-in", name: "Focusrite Scarlett 2i2", isDefault: false },
];
const outputDevices = [
  { id: "default-out", name: "MacBook Pro Speakers", isDefault: true },
  { id: "usb-out", name: "Focusrite Scarlett 2i2", isDefault: false },
];
const channelOptions = [
  { value: "1", label: "Ch 1" },
  { value: "2", label: "Ch 2" },
];
const sampleRateOptions = [
  { value: "44100", label: "44100 Hz" },
  { value: "48000", label: "48000 Hz" },
  { value: "96000", label: "96000 Hz" },
];
const bufferSizeOptions = [
  { value: "32", label: "32" },
  { value: "64", label: "64" },
  { value: "128", label: "128" },
  { value: "256", label: "256" },
];
const transmitChannelOptions = [
  { value: "1", label: "モノラル" },
  { value: "2", label: "ステレオ" },
];

const baseArgs = {
  inputDevices,
  outputDevices,
  selectedInputId: "default-in",
  selectedOutputId: "default-out",
  inputChannelOptions: channelOptions,
  outputChannelOptions: channelOptions,
  selectedInputChannelL: "1",
  selectedInputChannelR: "2",
  selectedOutputChannelL: "1",
  selectedOutputChannelR: "2",
  sampleRateOptions,
  selectedSampleRate: "48000",
  bufferSizeOptions,
  selectedBufferSize: "64",
  transmitChannelOptions,
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

export const Default: Story = {
  args: baseArgs,
};

export const Empty: Story = {
  args: {
    ...baseArgs,
    inputDevices: [],
    outputDevices: [],
    selectedInputId: null,
    selectedOutputId: null,
  },
};

export const Loading: Story = {
  args: {
    ...baseArgs,
    isLoading: true,
  },
};

export const BluetoothSelected: Story = {
  args: {
    ...baseArgs,
    inputDevices: [...inputDevices, { id: "bt-in", name: "Taro's AirPods", isBluetooth: true }],
    outputDevices: [...outputDevices, { id: "bt-out", name: "Taro's AirPods", isBluetooth: true }],
    selectedInputId: "bt-in",
    selectedOutputId: "bt-out",
  },
};
