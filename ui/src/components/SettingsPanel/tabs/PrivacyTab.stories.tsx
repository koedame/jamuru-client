import type { Meta, StoryObj } from "@storybook/react-vite";
import { PrivacyTab } from "./PrivacyTab";

const meta = {
  title: "Components/Settings/Tabs/PrivacyTab",
  component: PrivacyTab,
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
} satisfies Meta<typeof PrivacyTab>;

export default meta;
type Story = StoryObj<typeof meta>;

export const UsageReportingOff: Story = {
  args: {
    usageReporting: false,
    onUsageReportingChange: () => {},
    onShowUsagePreview: () => {},
  },
};

export const UsageReportingOn: Story = {
  args: {
    usageReporting: true,
    onUsageReportingChange: () => {},
    onShowUsagePreview: () => {},
  },
};

export const UsageReportingPreview: Story = {
  args: {
    usageReporting: true,
    onUsageReportingChange: () => {},
    onShowUsagePreview: () => {},
    usagePreview: [
      '{"v":1,"ts":"2026-09-24T02:10:00Z","seq":1,"event":"app_start","install_id":"a91d5c0e7b3f4a68b2c1d0e9f8a7b6c5","launch_id":"7f3c9a1e2b4d4c6f8e0a1b2c3d4e5f60","session_id":null,"app_version":"0.1.0","os":"macos","arch":"aarch64","os_version":"14.6","cpu_cores":8,"ram_gb":16,"language":"ja","audio_host":"coreaudio","settings":{"preset":"low_latency","sample_rate":48000,"buffer_size":128,"transmit_channels":2}}',
      '{"v":1,"ts":"2026-09-24T02:10:00Z","seq":2,"event":"audio_env","install_id":"a91d5c0e7b3f4a68b2c1d0e9f8a7b6c5","launch_id":"7f3c9a1e2b4d4c6f8e0a1b2c3d4e5f60","session_id":null,"app_version":"0.1.0","os":"macos","arch":"aarch64","input":{"name":"Scarlett 2i2 USB","kind":"usb","channels":2,"sample_rates":[44100,48000,96000],"min_buffer_frames":32,"is_default":true},"output":{"name":"Taro\'s AirPods","kind":"bluetooth","channels":2,"sample_rates":[48000],"min_buffer_frames":128,"is_default":false}}',
    ].join("\n"),
  },
};

export const UsageReportingOffPreview: Story = {
  args: {
    usageReporting: false,
    onUsageReportingChange: () => {},
    onShowUsagePreview: () => {},
    usagePreview: "",
  },
};

/** A hang from last time is waiting for an answer. */
export const PreviousHang: Story = {
  args: {
    previousHang: { stage: "restart", stalled_ms: 9000 },
    onSendPreviousHang: () => {},
    usageReporting: false,
    onUsageReportingChange: () => {},
    onShowUsagePreview: () => {},
  },
};

export const PreviousHangWithLog: Story = {
  args: {
    previousHang: { stage: "device_open", stalled_ms: 9000 },
    previousHangAttachesLog: true,
    onSendPreviousHang: () => {},
    usageReporting: false,
    onUsageReportingChange: () => {},
    onShowUsagePreview: () => {},
  },
};
