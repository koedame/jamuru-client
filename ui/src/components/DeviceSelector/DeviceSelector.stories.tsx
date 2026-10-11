import type { Meta, StoryObj } from "@storybook/react-vite";
import { DeviceSelector } from "./DeviceSelector";

const meta = {
  title: "Components/DeviceSelector",
  component: DeviceSelector,
  parameters: {
    layout: "padded",
  },
  decorators: [
    (Story) => (
      <div style={{ width: "280px", background: "var(--color-bg-primary)", padding: "16px" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof DeviceSelector>;

export default meta;
type Story = StoryObj<typeof meta>;

const sampleDevices = [
  { id: "default", name: "MacBook Pro Microphone", supported_sample_rates: [48000], supported_channels: [1, 2], is_default: true, is_asio: false, is_bluetooth: false },
  { id: "usb-1", name: "Focusrite Scarlett 2i2", supported_sample_rates: [44100, 48000, 96000], supported_channels: [1, 2], is_default: false, is_asio: false, is_bluetooth: false },
];

export const Default: Story = {
  args: {
    type: "input",
    devices: sampleDevices,
    selectedDeviceId: "default",
    onDeviceChange: () => {},
  },
};

export const Output: Story = {
  args: {
    type: "output",
    devices: sampleDevices,
    selectedDeviceId: "usb-1",
    onDeviceChange: () => {},
  },
};

export const Empty: Story = {
  args: {
    type: "input",
    devices: [],
    selectedDeviceId: null,
    onDeviceChange: () => {},
  },
};

export const Loading: Story = {
  args: {
    type: "input",
    devices: [],
    selectedDeviceId: null,
    onDeviceChange: () => {},
    isLoading: true,
  },
};

export const Disabled: Story = {
  args: {
    type: "input",
    devices: sampleDevices,
    selectedDeviceId: "default",
    onDeviceChange: () => {},
    disabled: true,
  },
};
