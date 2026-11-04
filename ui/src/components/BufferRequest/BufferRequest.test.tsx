/**
 * Asks to buffer a participant's audio for longer, as the user who is asked
 * sees them (ADR-097): the question, taking up or turning down, putting the
 * buffer back, and the notices about the asks the user made. What the backend
 * decides is `src/network/buffer_request.rs`.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useEffect } from "react";
import "@testing-library/jest-dom/vitest";

import i18n from "../../i18n";
import type { BufferRequestEvent, BufferRequestSnapshot, Participant } from "../../lib/tauri";
import { ALLOW_DELAY_MS } from "../SettingsHelp/SettingsHelpQuestion";
import { useBufferRequests, type BufferRequests } from "./useBufferRequests";

const invoke = vi.hoisted(() => vi.fn());
const handlers = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string, handler: (event: { payload: unknown }) => void) => {
    handlers.set(name, handler);
    return Promise.resolve(() => handlers.delete(name));
  }),
}));

const CONN = 3;
const AKI: Participant = { id: "aki-id", name: "Aki", features: ["peer_message"] };
const TERMS = { max_ms10: 300, tolerance_tenths: 5, auto: true };
const APPLIED = { applied_ms10: 120, rounded: false, at_lowest: false, auto_running: true };

let calls: { command: string; args: unknown }[];
let snapshot: BufferRequestSnapshot;
let requests: BufferRequests;

function Harness({ connId = CONN }: { connId?: number | null }) {
  const r = useBufferRequests(connId, [AKI]);
  useEffect(() => {
    requests = r;
  });
  return (
    <>
      {r.toast}
      {r.overlays}
      <span data-testid="held">{r.heldFor("aki-id") ? "held" : "not held"}</span>
      <span data-testid="asked">{r.askedOf("aki-id")?.standing.state ?? "none"}</span>
    </>
  );
}

/** Hands the screen an event, as the backend's window event does. */
function send(event: BufferRequestEvent) {
  act(() => handlers.get("buffer-request")!({ payload: event }));
}

async function mount(connId: number | null = CONN) {
  render(<Harness connId={connId} />);
  await waitFor(() => expect(handlers.has("buffer-request")).toBe(true));
}

async function untilAcceptIsReady() {
  await act(() => new Promise((resolve) => setTimeout(resolve, ALLOW_DELAY_MS + 50)));
}

beforeEach(async () => {
  calls = [];
  snapshot = { policy: "ask", asked: [], prompts: [], held: [] };
  handlers.clear();
  invoke.mockReset();
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    calls.push({ command, args });
    return command === "buffer_request_snapshot" ? snapshot : undefined;
  });
  await i18n.changeLanguage("en");
});

afterEach(() => vi.useRealTimers());

describe("an ask arrives", () => {
  // Verifies: REQ-GUI-036
  it("a participant asks for a longer buffer, the user is asked in words that name them, the length and what changes", async () => {
    await mount();

    send({ type: "asked", peer: "aki-id", peer_name: "Aki", terms: TERMS });

    const question = screen.getByTestId("settings-help-question");
    expect(question).toHaveTextContent("Aki asks you to buffer their audio at 30ms");
    expect(question).toHaveTextContent("only");
    expect(calls.filter((c) => c.command === "buffer_request_answer")).toEqual([]);
  });

  // Verifies: REQ-GUI-036
  it("the user accepts, the answer names what was shown and the question goes once the app has taken it", async () => {
    await mount();
    send({ type: "asked", peer: "aki-id", peer_name: "Aki", terms: TERMS });
    await untilAcceptIsReady();

    fireEvent.click(screen.getByTestId("settings-help-allow"));

    expect(calls).toContainEqual({
      command: "buffer_request_answer",
      args: { connId: CONN, peerId: "aki-id", accept: true, terms: TERMS },
    });
    send({ type: "ask_gone", peer: "aki-id" });
    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
  });

  // Verifies: REQ-GUI-036
  it("the ask changes as the user accepts, the app refuses and the question stays for the new ask", async () => {
    await mount();
    const longer = { ...TERMS, max_ms10: 500 };
    send({ type: "asked", peer: "aki-id", peer_name: "Aki", terms: TERMS });
    await untilAcceptIsReady();
    invoke.mockImplementation(async (command: string, args?: unknown) => {
      calls.push({ command, args });
      if (command === "buffer_request_answer") throw new Error("The ask changed after it was shown");
      return command === "buffer_request_snapshot" ? snapshot : undefined;
    });

    fireEvent.click(screen.getByTestId("settings-help-allow"));
    send({ type: "asked", peer: "aki-id", peer_name: "Aki", terms: longer });

    expect(calls).toContainEqual({
      command: "buffer_request_answer",
      args: { connId: CONN, peerId: "aki-id", accept: true, terms: TERMS },
    });
    expect(screen.getByTestId("settings-help-question")).toHaveTextContent("50ms");
  });

  // Verifies: REQ-GUI-036
  it("the user turns it down, the refusal is sent and the question goes", async () => {
    await mount();
    send({ type: "asked", peer: "aki-id", peer_name: "Aki", terms: TERMS });

    fireEvent.click(screen.getByTestId("settings-help-decline"));

    expect(calls).toContainEqual({
      command: "buffer_request_answer",
      args: { connId: CONN, peerId: "aki-id", accept: false },
    });
    send({ type: "ask_gone", peer: "aki-id" });
    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
  });

  // Verifies: REQ-GUI-036
  it("the asker withdraws before the user answers, the question goes without an answer sent", async () => {
    await mount();
    send({ type: "asked", peer: "aki-id", peer_name: "Aki", terms: TERMS });

    send({ type: "ask_gone", peer: "aki-id" });

    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
    expect(calls.filter((c) => c.command === "buffer_request_answer")).toEqual([]);
  });

  // Verifies: REQ-GUI-036
  it("the window opened after the ask came, the question is there from the snapshot", async () => {
    snapshot.prompts = [{ peer_id: "aki-id", peer_name: "Aki", terms: TERMS }];

    await mount();

    expect(await screen.findByTestId("settings-help-question")).toHaveTextContent("Aki asks");
  });

  // Verifies: REQ-GUI-036
  it("two participants ask, they are asked about one after the other", async () => {
    await mount();
    send({ type: "asked", peer: "aki-id", peer_name: "Aki", terms: TERMS });
    send({ type: "asked", peer: "bo-id", peer_name: "Bo", terms: TERMS });
    expect(screen.getByTestId("settings-help-question")).toHaveTextContent("Aki asks");

    fireEvent.click(screen.getByTestId("settings-help-decline"));
    send({ type: "ask_gone", peer: "aki-id" });

    expect(screen.getByTestId("settings-help-question")).toHaveTextContent("Bo asks");
  });
});

describe("a buffer held because a participant asked", () => {
  // Verifies: REQ-GUI-036
  it("the ask is taken up, the buffer is shown as held for them until it is put back", async () => {
    await mount();
    send({ type: "held", peer: "aki-id", terms: TERMS, applied: APPLIED });
    expect(screen.getByTestId("held")).toHaveTextContent("held");
    expect(requests.heldFor("aki-id")).toEqual({ terms: TERMS, applied: APPLIED });

    await act(() => requests.restore("aki-id"));

    expect(calls).toContainEqual({ command: "buffer_request_restore", args: { connId: CONN, peerId: "aki-id" } });
    expect(screen.getByTestId("held")).toHaveTextContent("not held");
  });

  // Verifies: REQ-GUI-036
  it("the asker leaves, the buffer is no longer shown as held and nothing is said", async () => {
    await mount();
    send({ type: "held", peer: "aki-id", terms: TERMS, applied: APPLIED });

    send({ type: "released", peer: "aki-id", reason: "peer_left" });

    expect(screen.getByTestId("held")).toHaveTextContent("not held");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  // Verifies: REQ-GUI-036
  it("the asker takes the ask back, the buffer is released and the user is told", async () => {
    await mount();
    send({ type: "held", peer: "aki-id", terms: TERMS, applied: APPLIED });

    send({ type: "released", peer: "aki-id", reason: "withdrawn" });

    expect(screen.getByTestId("held")).toHaveTextContent("not held");
    expect(screen.getByRole("status")).toHaveTextContent("Aki took the buffer ask back");
  });

  // Verifies: REQ-GUI-036
  it("the window opened after the ask was taken up, the buffer is shown as held from the snapshot", async () => {
    snapshot.held = [{ peer_id: "aki-id", terms: TERMS, applied: APPLIED }];

    await mount();

    await waitFor(() => expect(screen.getByTestId("held")).toHaveTextContent("held"));
  });
});

describe("the asks the user made", () => {
  // Verifies: REQ-GUI-035
  it("the user asks, the ask is sent with the terms and shows as being sent", async () => {
    await mount();

    await act(() => requests.ask("aki-id", TERMS));

    expect(calls).toContainEqual({
      command: "buffer_request_ask",
      args: { connId: CONN, peerId: "aki-id", terms: TERMS },
    });
    expect(screen.getByTestId("asked")).toHaveTextContent("sending");
  });

  // Verifies: REQ-GUI-035
  it("the participant accepts, the user is told the length their audio is buffered at", async () => {
    await mount();
    await act(() => requests.ask("aki-id", TERMS));

    send({ type: "standing", peer: "aki-id", standing: { state: "granted", applied: APPLIED } });

    expect(screen.getByTestId("asked")).toHaveTextContent("granted");
    expect(screen.getByRole("status")).toHaveTextContent("Aki is buffering your audio at 12ms");
  });

  // Verifies: REQ-GUI-035
  it.each([
    ["declined", "Aki said no to your buffer ask"],
    ["declined_by_setting", "Aki's settings do not take buffer asks"],
    ["unreachable", "No answer from Aki to your buffer ask"],
    ["restored", "Aki put the buffer for your audio back"],
  ] as const)("the ask ends as %s, the user is told", async (state, text) => {
    await mount();
    await act(() => requests.ask("aki-id", TERMS));

    send({ type: "standing", peer: "aki-id", standing: { state } });

    expect(screen.getByRole("status")).toHaveTextContent(text);
  });

  // Verifies: REQ-GUI-035
  it("the notice has gone and the same standing is told again, the user is not told twice", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    await mount();
    await act(() => requests.ask("aki-id", TERMS));
    send({ type: "standing", peer: "aki-id", standing: { state: "granted", applied: APPLIED } });
    expect(screen.getByRole("status")).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(7000));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();

    send({ type: "standing", peer: "aki-id", standing: { state: "granted", applied: { ...APPLIED, applied_ms10: 100 } } });

    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  // Verifies: REQ-GUI-035
  it("the user takes the ask back, it is sent and nothing is shown of it", async () => {
    await mount();
    await act(() => requests.ask("aki-id", TERMS));

    await act(() => requests.withdraw("aki-id"));

    expect(calls).toContainEqual({ command: "buffer_request_withdraw", args: { connId: CONN, peerId: "aki-id" } });
    expect(screen.getByTestId("asked")).toHaveTextContent("none");
  });

  // Verifies: REQ-GUI-035
  it("the window opened after the ask was made, how it stands is there from the snapshot", async () => {
    snapshot.asked = [{ peer_id: "aki-id", terms: TERMS, standing: { state: "pending" } }];

    await mount();

    await waitFor(() => expect(screen.getByTestId("asked")).toHaveTextContent("pending"));
  });
});

describe("there is no room connection", () => {
  // Verifies: REQ-GUI-035
  it("the connection is not there, nothing can be asked and nothing is sent", async () => {
    await mount(null);

    expect(requests.canAsk).toBe(false);
    await act(() => requests.ask("aki-id", TERMS));

    expect(calls.filter((c) => c.command.startsWith("buffer_request"))).toEqual([]);
  });
});
