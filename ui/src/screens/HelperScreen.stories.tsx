/**
 * Screens/Helper - the window someone helping works in: the helped app's own
 * main screen, drawn from that app's state, with the helper's banner on top.
 */
import type { Meta, StoryObj } from "@storybook/react-vite";
import { withBackend } from "../stories/fakeBackend";
import { answers, inRoom } from "../stories/mainFixtures";
import { audioSettings } from "../components/SettingsPanel/audioSettingsFixture";
import { HelperScreen } from "./HelperScreen";

const meta = {
  title: "Screens/Helper",
  component: HelperScreen,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof HelperScreen>;

export default meta;
type Story = StoryObj<typeof meta>;

const helped = () =>
  withBackend({
    ...answers(inRoom(["Aki"])),
    help_window_info: { peer_name: "Aki" },
    settings_get: audioSettings({
      input_devices: [
        { id: "in-1", name: "Built-in Microphone", supported_sample_rates: [48000], supported_channels: [1, 2], is_default: true, is_asio: false, is_bluetooth: false },
      ],
      output_devices: [
        { id: "out-1", name: "Built-in Speakers", supported_sample_rates: [48000], supported_channels: [2], is_default: true, is_asio: false, is_bluetooth: false },
      ],
    }),
    help_presence: [],
  });

export const Helping: Story = { decorators: [helped()] };
