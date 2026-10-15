/**
 * The window the person being helped keeps beside the app (ADR-044 §5): what
 * the help did, in order, how long until it ends by itself, and - once it is
 * over - what it changed, with a way to put it back.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";

import i18n from "../i18n";
import en from "../../locales/en.json";
import type { HelpRecord, HelpUndoResult } from "../lib/tauri";
import { HelpLogScreen } from "./HelpLogScreen";

const invoke = vi.hoisted(() => vi.fn());
const listeners = vi.hoisted(() => new Map<string, Array<(event: { payload: unknown }) => void>>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, [...(listeners.get(name) ?? []), handler]);
    return Promise.resolve(() =>
      listeners.set(name, (listeners.get(name) ?? []).filter((held) => held !== handler))
    );
  }),
  emitTo: vi.fn(),
}));

let calls: string[];
let record: HelpRecord | null;
let undoResult: HelpUndoResult;

function recordOf(patch: Partial<HelpRecord>): HelpRecord {
  return {
    helper_name: "Aki",
    entries: [],
    changed: [],
    active: true,
    remaining_ms: 20 * 60 * 1000 - 500,
    can_undo: false,
    ...patch,
  };
}

function announce(name: string, payload?: unknown) {
  act(() => (listeners.get(name) ?? []).forEach((heard) => heard({ payload })));
}

beforeEach(async () => {
  calls = [];
  listeners.clear();
  record = recordOf({});
  undoResult = { restored: ["buffer_size"], failed: [] };
  invoke.mockReset();
  invoke.mockImplementation(async (command: string) => {
    calls.push(command);
    if (command === "help_record_get") return record;
    if (command === "help_undo") return undoResult;
    return undefined;
  });
  await i18n.changeLanguage("en");
});

describe("HelpLogScreen", () => {
  // Verifies: REQ-RMT-034
  it("there is no help to show, the window says so", async () => {
    record = null;

    render(<HelpLogScreen />);

    expect(await screen.findByTestId("settings-help-log-heading")).toHaveTextContent(en.settingsHelp.log.none);
  });

  // Verifies: REQ-RMT-034
  it("a help goes on, the window says who helps, when it ends by itself, and offers Stop", async () => {
    render(<HelpLogScreen />);

    expect(await screen.findByTestId("settings-help-log-heading")).toHaveTextContent("Aki is helping");
    expect(screen.getByTestId("settings-help-log-remaining")).toHaveTextContent(
      en.settingsHelp.helped.remaining.replace("{{time}}", "20:00")
    );
    expect(screen.getByTestId("settings-help-log-stop")).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-034
  it("the help has done things, they are listed in the order they happened and in plain words", async () => {
    record = recordOf({
      entries: [
        { at_ms: 1_000, kind: "started" },
        { at_ms: 2_000, kind: "setting_changed", setting: "buffer_size" },
        { at_ms: 3_000, kind: "unmuted" },
      ],
    });

    render(<HelpLogScreen />);

    const lines = await screen.findAllByRole("listitem");
    expect(lines.map((line) => line.textContent)).toEqual([
      expect.stringContaining("Aki started helping"),
      expect.stringContaining("Aki changed the Buffer Size setting"),
      expect.stringContaining("Aki unmuted the microphone"),
    ]);
  });

  // Verifies: REQ-RMT-034
  it("a line is announced while the window is open, it is added once even if announced twice", async () => {
    render(<HelpLogScreen />);
    await screen.findByTestId("settings-help-log-heading");

    announce("help:log", { at_ms: 5_000, kind: "muted" });
    announce("help:log", { at_ms: 5_000, kind: "muted" });

    expect(await screen.findAllByText(/Aki muted the microphone/)).toHaveLength(1);
  });

  // Verifies: REQ-RMT-035
  it("the help ends while the window is open, the window reads the record again and says it is over", async () => {
    render(<HelpLogScreen />);
    await screen.findByTestId("settings-help-log-stop");
    record = recordOf({ active: false, remaining_ms: null, changed: ["buffer_size"], can_undo: true });

    announce("help:record-changed");

    expect(await screen.findByTestId("settings-help-log-heading")).toHaveTextContent("The help from Aki is over");
    expect(screen.queryByTestId("settings-help-log-stop")).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-034
  it("Stop is pressed, the main window is asked to stop the help", async () => {
    render(<HelpLogScreen />);

    fireEvent.click(await screen.findByTestId("settings-help-log-stop"));

    await waitFor(() => expect(calls).toContain("help_stop_request"));
  });

  // Verifies: REQ-RMT-035
  it("the help is over and changed settings, the window says which and offers to put them back", async () => {
    record = recordOf({ active: false, remaining_ms: null, changed: ["buffer_size"], can_undo: true });

    render(<HelpLogScreen />);

    expect(await screen.findByTestId("settings-help-log-heading")).toHaveTextContent("The help from Aki is over");
    expect(screen.getByText("Aki changed: Buffer Size")).toBeInTheDocument();
    expect(screen.getByTestId("settings-help-log-undo")).toBeInTheDocument();
    expect(screen.queryByTestId("settings-help-log-stop")).not.toBeInTheDocument();
    expect(screen.queryByTestId("settings-help-log-remaining")).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-035
  it("Put back is pressed, the settings are put back once and the window says so", async () => {
    record = recordOf({ active: false, remaining_ms: null, changed: ["buffer_size"], can_undo: true });
    render(<HelpLogScreen />);

    fireEvent.click(await screen.findByTestId("settings-help-log-undo"));

    expect(await screen.findByTestId("settings-help-log-undone")).toHaveTextContent(en.settingsHelp.report.restored);
    expect(calls.filter((c) => c === "help_undo")).toHaveLength(1);
    expect(calls).toContain("help_record_dismiss");
  });

  // Verifies: REQ-RMT-035
  it("a setting cannot be put back, the window says which", async () => {
    record = recordOf({ active: false, remaining_ms: null, changed: ["input_device"], can_undo: true });
    undoResult = { restored: [], failed: [{ setting: "input_device", reason: "device_gone" }] };
    render(<HelpLogScreen />);

    fireEvent.click(await screen.findByTestId("settings-help-log-undo"));

    expect(await screen.findByTestId("settings-help-log-undone")).toHaveTextContent(
      en.settingsHelp.report.notRestored.replace("{{settings}}", "Input Device")
    );
    expect(calls).not.toContain("help_record_dismiss");
  });
});
