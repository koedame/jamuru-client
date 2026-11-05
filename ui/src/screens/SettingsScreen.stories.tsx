/**
 * Screens/Settings - the settings window, one story for each tab it can open
 * on (the URL names the tab). The window is the real `SettingsScreen` over a
 * fake backend.
 */
import type { Decorator, Meta, StoryObj } from "@storybook/react-vite";
import { audioSettings } from "../components/SettingsPanel/audioSettingsFixture";
import { withBackend } from "../stories/fakeBackend";
import { SettingsScreen } from "./SettingsScreen";

const meta = {
  title: "Screens/Settings",
  component: SettingsScreen,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof SettingsScreen>;

export default meta;
type Story = StoryObj<typeof meta>;

const device = (id: string, name: string, isDefault: boolean) => ({
  id,
  name,
  supported_sample_rates: [44100, 48000, 96000],
  supported_channels: [1, 2],
  is_default: isDefault,
  is_asio: false,
  is_bluetooth: false,
});

const backend = withBackend({
  terms_get: { version: 1, accepted: true, text: "# Terms" },
  config_load: { usage_reporting: false, buffer_size: 64 },
  settings_get: audioSettings({
    input_devices: [device("in-1", "MacBook Pro Microphone", true), device("in-2", "Focusrite Scarlett 2i2", false)],
    output_devices: [device("out-1", "MacBook Pro Speakers", true), device("out-2", "Focusrite Scarlett 2i2", false)],
    input_channel_count: 2,
    output_channel_count: 2,
  }),
  config_get_peer_name: "Yuki",
  update_status: { current: "0.9.0", support: "install" },
  update_check: { kind: "up_to_date" },
  window_is_in_session: false,
  delay_measure_status: {
    session_running: false,
    running: false,
    stimulus: null,
    step: null,
    steps: 6,
    failure: null,
    figure: null,
    peers: {},
  },
});

/** Opens the window as its URL would: on `tab` */
const onTab =
  (tab: string): Decorator =>
  (Story) => {
    window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}#/settings/${tab}`);
    return <Story />;
  };

export const General: Story = { decorators: [backend, onTab("general")] };
export const Devices: Story = { decorators: [backend, onTab("devices")] };
export const Privacy: Story = { decorators: [backend, onTab("privacy")] };
export const Diagnostics: Story = { decorators: [backend, onTab("diagnostics")] };
export const About: Story = { decorators: [backend, onTab("about")] };
