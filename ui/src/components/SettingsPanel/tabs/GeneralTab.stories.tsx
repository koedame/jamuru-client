import type { Meta, StoryObj } from "@storybook/react-vite";
import { GeneralTab } from "./GeneralTab";

const meta = {
  title: "Components/Settings/Tabs/GeneralTab",
  component: GeneralTab,
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
} satisfies Meta<typeof GeneralTab>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Japanese: Story = {
  args: {
    displayName: "ユーザー",
    onDisplayNameChange: () => {},
    askNameOnEntry: true,
    onAskNameOnEntryChange: () => {},
    language: "ja",
    onLanguageChange: () => {},
    serverUrl: "",
    effectiveServerUrl: "signaling.example.com",
    onServerUrlChange: () => {},
  },
};

export const English: Story = {
  args: {
    displayName: "User",
    onDisplayNameChange: () => {},
    language: "en",
    onLanguageChange: () => {},
    serverUrl: "",
    effectiveServerUrl: "signaling.example.com",
    onServerUrlChange: () => {},
  },
};

/** A leftover dev/test URL is still configured - the hint calls it out. */
export const CustomServer: Story = {
  args: {
    displayName: "User",
    onDisplayNameChange: () => {},
    language: "en",
    onLanguageChange: () => {},
    serverUrl: "signaling.example.com",
    effectiveServerUrl: "signaling.example.com",
    onServerUrlChange: () => {},
  },
};

export const DisplayNameError: Story = {
  args: {
    displayName: "",
    displayNameError: "Display name is required",
    onDisplayNameChange: () => {},
    language: "en",
    onLanguageChange: () => {},
    serverUrl: "",
    effectiveServerUrl: "signaling.example.com",
    onServerUrlChange: () => {},
  },
};

/** The name check before entering a room is turned off. */
export const NameNotAsked: Story = {
  args: {
    displayName: "User",
    onDisplayNameChange: () => {},
    askNameOnEntry: false,
    onAskNameOnEntryChange: () => {},
    language: "en",
    onLanguageChange: () => {},
    serverUrl: "",
    effectiveServerUrl: "signaling.example.com",
    onServerUrlChange: () => {},
  },
};
