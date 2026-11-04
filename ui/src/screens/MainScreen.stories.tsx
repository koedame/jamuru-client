/**
 * Screens/Main - the main window, drawn by the real `MainScreen` from a fake
 * backend (`stories/fakeBackend.tsx`). Each story is one thing the backend can
 * say about the session.
 */
import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";
import { withBackend, type Answers } from "../stories/fakeBackend";
import { answers, idleRecording, inRoom, snapshot } from "../stories/mainFixtures";
import type { SessionSnapshot } from "../lib/tauri";
import { MainScreen } from "./MainScreen";

const meta = {
  title: "Screens/Main",
  component: MainScreen,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof MainScreen>;

export default meta;
type Story = StoryObj<typeof meta>;

const withSession = (session: SessionSnapshot, extra?: Answers) => withBackend(answers(session, extra));

export const ConnectingToServer: Story = {
  decorators: [withSession(snapshot({ phase: "connecting_server", connection_id: null }))],
};

export const ServerError: Story = {
  decorators: [withSession(snapshot({ phase: "error", connection_id: null, error: "connection refused" }))],
};

export const Lobby: Story = { decorators: [withSession(snapshot())] };

export const LobbyWithTestRoom: Story = {
  decorators: [withSession(snapshot({ test_room_invite_code: "HJK567MNP" }))],
};

export const Joining: Story = {
  decorators: [withSession(snapshot({ phase: "joining", joining_code: "ABC234XYZ" }))],
};

export const AskNameBeforeEntering: Story = {
  decorators: [withSession(snapshot(), { config_load: { peer_name: "Yuki", ask_name_on_entry: true } })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByTestId("connection-panel-create-room"));
  },
};

export const InRoomAlone: Story = { decorators: [withSession(inRoom([]))] };

export const InRoomWithOne: Story = { decorators: [withSession(inRoom(["Aki"]))] };

export const InRoomWithThree: Story = { decorators: [withSession(inRoom(["Aki", "Ben", "Chika"]))] };

export const Recording: Story = {
  decorators: [
    withSession(inRoom(["Aki", "Ben"]), {
      recording_status: { ...idleRecording, recording: true, seconds: 83, file_name: "session.wav" },
    }),
  ],
};

export const UpdateAvailable: Story = {
  decorators: [withSession(inRoom(["Aki"]), { update_notice_get: "0.10.0" })],
};

export const ReconnectingToServer: Story = {
  decorators: [withSession(inRoom(["Aki"], { signaling_reconnect: "reconnecting" }))],
};

export const ReconnectFailed: Story = {
  decorators: [
    withSession(inRoom(["Aki"], { signaling_reconnect: "failed", signaling_reconnect_error: "network unreachable" })),
  ],
};

export const LeaveDialog: Story = {
  decorators: [withSession(inRoom(["Aki"]))],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByTestId("leave-room"));
    await waitFor(() => expect(canvas.queryByRole("dialog")).not.toBeNull());
  },
};
