/**
 * What the person being helped is told while a help goes on and after it
 * (ADR-044 §5): how long until it ends by itself, what the helper did to the
 * microphone, and - once it is over - what it changed, with a way to put it
 * back. The record itself is kept by the app (`src-tauri/src/help_record.rs`).
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useEffect } from "react";

import i18n from "../../i18n";
import en from "../../../locales/en.json";
import type { HelpEvent, HelpRecord, HelpUndoResult } from "../../lib/tauri";
import { useSettingsHelp, type SettingsHelp } from "./useSettingsHelp";

const invoke = vi.hoisted(() => vi.fn());
/** Who listens for each event name */
const listeners = vi.hoisted(() => new Map<string, Array<(event: { payload: unknown }) => void>>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, [...(listeners.get(name) ?? []), handler]);
    return Promise.resolve(() =>
      listeners.set(name, (listeners.get(name) ?? []).filter((held) => held !== handler))
    );
  }),
}));

const CONN = 7;
/** Counts are rounded up, so a count that has just started reads as a whole minute */
const JUST_UNDER = 500;
const AKI_HELPS: HelpEvent = { type: "started", role: "helped", peer: "aki-id", peer_name: "Aki" };

let calls: string[];
let record: HelpRecord | null;
let undoResult: HelpUndoResult;

function recordOf(patch: Partial<HelpRecord>): HelpRecord {
  return {
    helper_name: "Aki",
    entries: [],
    changed: [],
    active: true,
    remaining_ms: 30 * 60 * 1000 - JUST_UNDER,
    can_undo: false,
    ...patch,
  };
}

function mount() {
  let help: SettingsHelp | undefined;
  function Harness() {
    const h = useSettingsHelp(CONN, []);
    useEffect(() => {
      help = h;
    });
    return (
      <>
        {h.bars}
        {h.overlays}
      </>
    );
  }
  render(<Harness />);
  return {
    send: (event: HelpEvent) =>
      act(() => {
        help!.onEvent(event);
      }),
  };
}

/** The app announces `name` to every window. */
function announce(name: string, payload?: unknown) {
  act(() => (listeners.get(name) ?? []).forEach((heard) => heard({ payload })));
}

/** A help by Aki that is going on, as the person sees it. */
async function beingHelped() {
  const screenOf = mount();
  screenOf.send(AKI_HELPS);
  await screen.findByTestId("settings-help-stop-helped");
  return screenOf;
}

beforeEach(async () => {
  calls = [];
  listeners.clear();
  record = recordOf({});
  undoResult = { restored: ["input_device", "buffer_size"], failed: [] };
  invoke.mockReset();
  invoke.mockImplementation(async (command: string) => {
    calls.push(command);
    if (command === "help_record_get") return record;
    if (command === "help_undo") return undoResult;
    return undefined;
  });
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.useRealTimers();
});

function remainingText(time: string) {
  return en.settingsHelp.helped.remaining.replace("{{time}}", time);
}

describe("the time left", () => {
  // Verifies: REQ-RMT-036
  it("the help has started, the bar says how long until it ends by itself", async () => {
    await beingHelped();

    expect(await screen.findByTestId("settings-help-status")).toHaveTextContent(remainingText("30:00"));
  });

  // Verifies: REQ-RMT-036
  it("the app says there is less time left, the bar follows it", async () => {
    await beingHelped();
    await screen.findByTestId("settings-help-status");

    announce("help:remaining", { remaining_ms: 10 * 60 * 1000 - JUST_UNDER });

    expect(screen.getByTestId("settings-help-status")).toHaveTextContent(remainingText("10:00"));
  });

  // Verifies: REQ-RMT-036
  it("a few seconds pass with nothing announced, the count runs down by itself", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    await beingHelped();
    await screen.findByTestId("settings-help-status");

    act(() => {
      vi.advanceTimersByTime(5000);
    });

    expect(screen.getByTestId("settings-help-status")).toHaveTextContent(remainingText("29:55"));
  });

  // Verifies: REQ-RMT-036
  it("the count is not read out as it changes, since it would be every second", async () => {
    await beingHelped();

    expect(await screen.findByTestId("settings-help-status")).toHaveAttribute("aria-live", "off");
  });

  // Verifies: REQ-RMT-036
  it("the help ended for sitting idle, the person is told it ended and why", async () => {
    const { send } = await beingHelped();

    send({ type: "ended", role: "helped", peer: "aki-id", reason: "timed_out" });

    expect(await screen.findByText(en.settingsHelp.ended.timedOut.replace("{{name}}", "Aki"))).toBeInTheDocument();
    expect(screen.queryByTestId("settings-help-bar")).not.toBeInTheDocument();
  });
});

describe("what the helper does to the microphone", () => {
  // Verifies: REQ-RMT-034
  it("the helper unmutes the microphone, the person is told at once", async () => {
    await beingHelped();

    announce("help:log", { at_ms: 1, kind: "unmuted" });

    expect(await screen.findByText(en.settingsHelp.log.unmuted.replace("{{name}}", "Aki"))).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-034
  it("the helper mutes the microphone, the person is told too", async () => {
    await beingHelped();

    announce("help:log", { at_ms: 1, kind: "muted" });

    expect(await screen.findByText(en.settingsHelp.log.muted.replace("{{name}}", "Aki"))).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-034
  it("a setting is changed, the chat has it and nothing else is put up over the screen", async () => {
    await beingHelped();

    announce("help:log", { at_ms: 1, kind: "setting_changed", setting: "buffer_size" });

    expect(screen.queryByRole("status", { name: /changed/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/changed the/)).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-034
  it("no help is going on, a line of the record put up nothing", () => {
    mount();

    announce("help:log", { at_ms: 1, kind: "unmuted" });

    expect(screen.queryByText(/unmuted/)).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-034
  it("the person opens the log, the log window opens", async () => {
    await beingHelped();

    fireEvent.click(screen.getByTestId("settings-help-log-open"));

    expect(calls).toContain("help_log_window_open");
  });

  // Verifies: REQ-RMT-034
  it("Stop is pressed in the log window, the help is stopped from here", async () => {
    await beingHelped();

    announce("help:stop-requested");

    await waitFor(() => expect(calls).toContain("settings_help_stop"));
    expect(screen.queryByTestId("settings-help-stop-helped")).not.toBeInTheDocument();
  });
});

describe("after the help", () => {
  const changedRecord = () =>
    recordOf({ active: false, remaining_ms: null, changed: ["input_device", "buffer_size"], can_undo: true });

  // Verifies: REQ-RMT-035
  it("the person stops a help that changed settings, they are told which and can put them back", async () => {
    await beingHelped();
    record = changedRecord();

    fireEvent.click(screen.getByTestId("settings-help-stop-helped"));

    const bar = await screen.findByText(/Aki changed:/);
    expect(bar).toHaveTextContent("Aki changed: Input Device, Buffer Size");
    expect(screen.getByTestId("settings-help-undo")).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-035
  it("the help ended by itself having changed settings, they are told which as well", async () => {
    const { send } = await beingHelped();
    record = changedRecord();

    send({ type: "ended", role: "helped", peer: "aki-id", reason: "peer_stopped" });

    expect(await screen.findByText(/Aki changed:/)).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-035
  it("the person puts the settings back, it is done in one press and they are told", async () => {
    await beingHelped();
    record = changedRecord();
    fireEvent.click(screen.getByTestId("settings-help-stop-helped"));

    fireEvent.click(await screen.findByTestId("settings-help-undo"));

    expect(await screen.findByText(en.settingsHelp.report.restored)).toBeInTheDocument();
    expect(calls.filter((c) => c === "help_undo")).toHaveLength(1);
    expect(screen.queryByText(/Aki changed:/)).not.toBeInTheDocument();
    expect(calls).toContain("help_record_dismiss");
  });

  // Verifies: REQ-RMT-035
  it("a setting cannot be put back, it stays listed to try again and the person is told which", async () => {
    await beingHelped();
    record = changedRecord();
    undoResult = { restored: ["buffer_size"], failed: [{ setting: "input_device", reason: "device_gone" }] };
    fireEvent.click(screen.getByTestId("settings-help-stop-helped"));

    fireEvent.click(await screen.findByTestId("settings-help-undo"));

    expect(
      await screen.findByText(en.settingsHelp.report.notRestored.replace("{{settings}}", "Input Device"))
    ).toBeInTheDocument();
    expect(screen.getByTestId("settings-help-undo")).toBeInTheDocument();
    expect(screen.getByText("Aki changed: Input Device")).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-035
  it("how the settings were is not known, they are listed without a way to put them back", async () => {
    await beingHelped();
    record = { ...changedRecord(), can_undo: false };

    fireEvent.click(screen.getByTestId("settings-help-stop-helped"));

    await screen.findByText(/Aki changed:/);
    expect(screen.queryByTestId("settings-help-undo")).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-035
  it("the person closes the report, it is gone and the record is cleared", async () => {
    await beingHelped();
    record = changedRecord();
    fireEvent.click(screen.getByTestId("settings-help-stop-helped"));

    fireEvent.click(await screen.findByTestId("settings-help-report-dismiss"));

    expect(screen.queryByText(/Aki changed:/)).not.toBeInTheDocument();
    expect(calls).toContain("help_record_dismiss");
  });

  // Verifies: REQ-RMT-035
  it("the help changed nothing, there is no report and the record is cleared", async () => {
    await beingHelped();
    record = recordOf({ active: false, remaining_ms: null });

    fireEvent.click(screen.getByTestId("settings-help-stop-helped"));

    await waitFor(() => expect(calls).toContain("help_record_dismiss"));
    expect(screen.queryByText(/changed:/)).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-035
  it("a new help begins, the report of the last one goes away", async () => {
    const { send } = await beingHelped();
    record = changedRecord();
    fireEvent.click(screen.getByTestId("settings-help-stop-helped"));
    await screen.findByText(/Aki changed:/);

    send({ type: "started", role: "helped", peer: "bo-id", peer_name: "Bo" });

    await waitFor(() => expect(screen.queryByText(/Aki changed:/)).not.toBeInTheDocument());
  });
});
