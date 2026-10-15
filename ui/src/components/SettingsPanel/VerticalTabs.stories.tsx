import type { Meta, StoryObj } from "@storybook/react-vite";
import { VerticalTabs, Tab } from "./VerticalTabs";
import { MicIcon, SlidersIcon, ShieldIcon, ActivityIcon, InfoIcon } from "./icons";

const meta = {
  title: "Components/Settings/VerticalTabs",
  component: VerticalTabs,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div style={{ height: "300px", display: "flex" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof VerticalTabs>;

export default meta;
type Story = StoryObj<typeof meta>;

const defaultTabs: Tab[] = [
  { id: "general", label: "General", icon: <SlidersIcon /> },
  { id: "devices", label: "Audio", icon: <MicIcon /> },
  { id: "privacy", label: "Privacy", icon: <ShieldIcon /> },
  { id: "diagnostics", label: "Troubleshooting", icon: <ActivityIcon /> },
  { id: "about", label: "About", icon: <InfoIcon /> },
];

const japaneseTabs: Tab[] = [
  { id: "general", label: "一般", icon: <SlidersIcon /> },
  { id: "devices", label: "オーディオ", icon: <MicIcon /> },
  { id: "privacy", label: "プライバシー", icon: <ShieldIcon /> },
  { id: "diagnostics", label: "診断とサポート", icon: <ActivityIcon /> },
  { id: "about", label: "アプリについて", icon: <InfoIcon /> },
];

export const Default: Story = {
  args: {
    tabs: defaultTabs,
    selectedId: "devices",
    onSelect: () => {},
  },
};

export const PrivacySelected: Story = {
  args: {
    tabs: defaultTabs,
    selectedId: "privacy",
    onSelect: () => {},
  },
};

export const DevicesSelected: Story = {
  args: {
    tabs: defaultTabs,
    selectedId: "devices",
    onSelect: () => {},
  },
};

export const JapaneseTabs: Story = {
  args: {
    tabs: japaneseTabs,
    selectedId: "devices",
    onSelect: () => {},
  },
};

export const TwoTabs: Story = {
  args: {
    tabs: [
      { id: "audio", label: "Audio" },
      { id: "video", label: "Video" },
    ],
    selectedId: "audio",
    onSelect: () => {},
  },
};
