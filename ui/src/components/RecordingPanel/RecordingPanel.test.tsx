/**
 * Recording the room in the main window (ADR-095): the card in the sidebar,
 * the mark beside a participant who records, the chat line, and the notice of
 * a recording that ended by itself.
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, render, screen, fireEvent, waitFor } from "@testing-library/react";

import i18n from "../../i18n";
import en from "../../../locales/en.json";
import ja from "../../../locales/ja.json";
import type { RecordingEnded, RecordingStatus } from "../../lib/tauri";
import { ChatMessage } from "../ChatPanel/ChatMessage";
import { RecordingMark, RecordingPanel, formatDuration, type RecordingPanelProps } from "./RecordingPanel";
import { RECORDING_POLL_MS, useRoomRecording } from "./useRoomRecording";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

function ended(by: RecordingEnded["ended_by"], more: Partial<RecordingEnded> = {}): RecordingEnded {
  return {
    id: 1,
    file_name: "jamuru-2026-11-04-153012.wav",
    self_file_name: null,
    seconds: 75,
    dropped_frames: 0,
    ended_by: by,
    error: null,
    ...more,
  };
}

function panel(more: Partial<RecordingPanelProps> = {}) {
  const props: RecordingPanelProps = {
    recording: false,
    seconds: 0,
    fileName: null,
    withSelf: false,
    last: null,
    onWithSelfChange: vi.fn(),
    onStart: vi.fn(),
    onStop: vi.fn(),
    onOpenFolder: vi.fn(),
    ...more,
  };
  render(<RecordingPanel {...props} />);
  return props;
}

beforeEach(async () => {
  invoke.mockReset();
  await i18n.changeLanguage("en");
});

describe("the recording card", () => {
  // Verifies: REQ-GUI-032
  it("nothing records, the card offers to start and says everyone is told", () => {
    const props = panel();

    expect(screen.getByTestId("recording-panel")).toHaveAttribute("data-recording", "false");
    expect(screen.getByTestId("recording-panel")).toHaveTextContent(en.recording.notice);
    fireEvent.click(screen.getByTestId("recording-start"));
    expect(props.onStart).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId("recording-stop")).not.toBeInTheDocument();
  });

  // Verifies: REQ-GUI-032
  it("the microphone's own file is asked for with the checkbox", () => {
    const props = panel();

    fireEvent.click(screen.getByRole("checkbox", { name: en.recording.withSelf }));

    expect(props.onWithSelfChange).toHaveBeenCalledWith(true);
  });

  // Verifies: REQ-GUI-032
  it("a recording runs, the card shows the time and the file and offers to stop", () => {
    const props = panel({ recording: true, seconds: 75, fileName: "jamuru-x.wav" });

    expect(screen.getByTestId("recording-panel")).toHaveAttribute("data-recording", "true");
    expect(screen.getByTestId("recording-time")).toHaveTextContent("1:15");
    expect(screen.getByTestId("recording-panel")).toHaveTextContent("jamuru-x.wav");
    expect(screen.queryByTestId("recording-start")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("recording-stop"));
    expect(props.onStop).toHaveBeenCalledTimes(1);
  });

  // Verifies: REQ-GUI-032
  it("a recording that the person stopped, the card says it is saved, where, and opens the folder", () => {
    const props = panel({ last: ended("stopped") });

    const result = screen.getByTestId("recording-result");
    expect(result).toHaveTextContent("Saved (1:15)");
    expect(result).toHaveTextContent("jamuru-2026-11-04-153012.wav");
    fireEvent.click(screen.getByTestId("recording-open-folder"));
    expect(props.onOpenFolder).toHaveBeenCalledTimes(1);
  });

  // Verifies: REQ-GUI-032
  it.each([
    ["session_ended", en.recording.ended.sessionEnded],
    ["size_limit", en.recording.ended.sizeLimit],
    ["write_failed", en.recording.ended.writeFailed],
  ] as const)("a recording that ended as %s, the card says why", (by, text) => {
    panel({ last: ended(by) });

    expect(screen.getByTestId("recording-result")).toHaveTextContent(text);
  });

  // Verifies: REQ-GUI-032
  it("frames were lost to a slow disk, the card says part of the sound is missing", () => {
    panel({ last: ended("stopped", { dropped_frames: 4800 }) });

    expect(screen.getByTestId("recording-dropped")).toHaveTextContent(en.recording.ended.dropped);
  });

  // Verifies: REQ-GUI-032
  it("a start fails, the card shows what the app said", () => {
    panel({ error: "A recording is already running" });

    expect(screen.getByTestId("recording-error")).toHaveTextContent("A recording is already running");
  });

  // Verifies: REQ-GUI-032
  it("the time is shown as minutes and seconds, and with hours past an hour", () => {
    expect(formatDuration(0)).toBe("0:00");
    expect(formatDuration(75.9)).toBe("1:15");
    expect(formatDuration(3725)).toBe("1:02:05");
  });

  // Verifies: REQ-CON-034
  it("a participant records, the mark beside them says so in the UI language", async () => {
    render(<RecordingMark />);
    expect(screen.getByTestId("recording-mark")).toHaveTextContent(en.recording.on);

    await act(() => i18n.changeLanguage("ja"));
    expect(screen.getByTestId("recording-mark")).toHaveTextContent(ja.recording.on);
  });
});

describe("the chat line for someone recording", () => {
  // Verifies: REQ-CON-034
  it.each([
    ["recording_started", en.chat.system.recordingStarted],
    ["recording_stopped", en.chat.system.recordingStopped],
  ] as const)("a %s line names who, in the UI language", async (kind, template) => {
    render(
      <ChatMessage type="system" senderName="Aki" content="" timestamp={0} systemKind={kind} />
    );

    expect(screen.getByText(template.replace("{{name}}", "Aki"))).toBeInTheDocument();
    expect(document.querySelector('[data-system-kind="' + kind + '"]')).not.toBeNull();
  });
});

function status(more: Partial<RecordingStatus> = {}): RecordingStatus {
  return {
    recording: false,
    seconds: 0,
    dropped_frames: 0,
    file_name: null,
    last: null,
    peers: [],
    ...more,
  };
}

function Harness({ active }: { active: boolean }) {
  const recording = useRoomRecording(active);
  return (
    <>
      <span data-testid="peers">{recording.status.peers.join(",")}</span>
      <span data-testid="on">{String(recording.status.recording)}</span>
      <button onClick={recording.start}>start</button>
      <span data-testid="error">{recording.error}</span>
      {recording.toast}
    </>
  );
}

describe("reading the recording", () => {
  // Verifies: REQ-CON-034
  it("a session is up, who records is read again and again", async () => {
    let peers: string[] = [];
    invoke.mockImplementation(async () => status({ peers }));
    render(<Harness active />);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("recording_status", undefined, undefined));

    peers = ["aki-id"];

    await waitFor(() => expect(screen.getByTestId("peers")).toHaveTextContent("aki-id"), {
      timeout: RECORDING_POLL_MS * 4,
    });
  });

  // Verifies: REQ-AUD-047
  it("the room is left and the recording ended with it, the person is told where it was saved", async () => {
    invoke.mockImplementation(async () => status({ last: null }));
    const { rerender } = render(<Harness active />);
    await waitFor(() => expect(invoke).toHaveBeenCalled());

    invoke.mockImplementation(async () => status({ last: ended("session_ended") }));
    rerender(<Harness active={false} />);

    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent(
        `${en.recording.ended.sessionEnded} (jamuru-2026-11-04-153012.wav)`
      )
    );
  });

  // Verifies: REQ-AUD-047
  it("an ending from before this window opened is not announced", async () => {
    invoke.mockImplementation(async () => status({ last: ended("session_ended") }));
    render(<Harness active />);
    await waitFor(() => expect(invoke).toHaveBeenCalled());
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));

    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  // Verifies: REQ-AUD-047
  it("the person stopped it themselves, no notice is added to the card that says so", async () => {
    invoke.mockImplementation(async () => status({ last: null }));
    const { rerender } = render(<Harness active />);
    await waitFor(() => expect(invoke).toHaveBeenCalled());

    invoke.mockImplementation(async () => status({ last: ended("stopped") }));
    rerender(<Harness active={false} />);
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));

    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  // Verifies: REQ-AUD-046
  it("a start is refused, the person sees why and nothing is marked as recording", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "recording_start") throw "There is no audio to record: the room has not been entered";
      return status();
    });
    render(<Harness active />);

    fireEvent.click(screen.getByText("start"));

    await waitFor(() => expect(screen.getByTestId("error")).toHaveTextContent("There is no audio to record"));
    expect(screen.getByTestId("on")).toHaveTextContent("false");
  });
});
