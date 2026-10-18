/**
 * Helping with settings in the main window (ADR-044 §5): asking someone for
 * help, the question the helper answers, the bars that say a help is going on,
 * and stopping. The app's side (what is sent, when the help ends) is
 * `src-tauri/src/settings_help.rs`; what the helper does is in their own window
 * (`HelperScreen`).
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { useEffect } from "react";

import i18n from "../../i18n";
import en from "../../../locales/en.json";
import ja from "../../../locales/ja.json";
import type { HelpEvent, Participant } from "../../lib/tauri";
import { ChatMessage } from "../ChatPanel/ChatMessage";
import { ALLOW_DELAY_MS } from "./SettingsHelpQuestion";
import { ASK_WAIT_MS, useSettingsHelp, type SettingsHelp } from "./useSettingsHelp";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

const CONN = 7;

function peer(id: string, name: string, features: string[] = ["peer_message"]): Participant {
  return { id, name, features };
}

const AKI = peer("aki-id", "Aki");
const BO = peer("bo-id", "Bo");

/** Mounts the hook as the main window does, and hands back its handle. */
function mount(participants: Participant[]) {
  return mountWith(() => participants);
}

/** `mount`, with the participants read on each render (`rerender` after changing them). */
function mountWith(participantsNow: () => Participant[]) {
  let help: SettingsHelp | undefined;
  function Harness() {
    const participants = participantsNow();
    const h = useSettingsHelp(CONN, participants);
    useEffect(() => {
      help = h;
    });
    return (
      <>
        {h.canAsk && <button onClick={h.openAsk}>ask for help</button>}
        {h.bars}
        {h.toast}
        {h.overlays}
      </>
    );
  }
  const { rerender } = render(<Harness />);
  return {
    send: (event: HelpEvent) =>
      act(() => {
        help!.onEvent(event);
      }),
    /** Several events handed over in one go, as one poll delivers them */
    sendAll: (events: HelpEvent[]) =>
      act(() => {
        for (const event of events) help!.onEvent(event);
      }),
    rerender: () => rerender(<Harness />),
  };
}

/** Commands the window sent, as `{ command, args }`. */
let calls: { command: string; args: unknown }[];

function fill(template: string, values: Record<string, string | number>): string {
  return template.replace(/{{(\w+)}}/g, (_, key) => String(values[key]));
}

/** Waits until a question that has just appeared lets Allow through. */
async function untilAllowIsReady() {
  await act(() => new Promise((resolve) => setTimeout(resolve, ALLOW_DELAY_MS + 50)));
}

/** Asks `name` for help through the dialog, as a person would. */
async function askFor(name: string) {
  fireEvent.click(screen.getByRole("button", { name: "ask for help" }));
  fireEvent.click(screen.getByTestId("settings-help-ask-next"));
  fireEvent.click(screen.getByRole("radio", { name }));
  fireEvent.click(screen.getByTestId("settings-help-ask-send"));
  await waitFor(() => expect(screen.getByTestId("settings-help-bar")).toBeInTheDocument());
}

beforeEach(async () => {
  calls = [];
  invoke.mockReset();
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    calls.push({ command, args });
    return undefined;
  });
  await i18n.changeLanguage("en");
});

describe("asking for help", () => {
  // Verifies: REQ-RMT-001
  it("the person opens the ask, it says what it means in a few lines and nothing is sent yet", () => {
    mount([AKI]);

    fireEvent.click(screen.getByRole("button", { name: "ask for help" }));

    const dialog = screen.getByTestId("settings-help-ask");
    expect(dialog).toHaveTextContent(en.settingsHelp.ask.title);
    expect(dialog).toHaveTextContent(en.settingsHelp.ask.limits);
    expect(dialog).toHaveTextContent(en.settingsHelp.ask.control);
    expect(calls).toEqual([]);
  });

  // Verifies: REQ-RMT-007
  it("choosing who to ask lists the participants whose app takes peer messages, and not the others", () => {
    mount([AKI, peer("old-id", "Old app", [])]);

    fireEvent.click(screen.getByRole("button", { name: "ask for help" }));
    fireEvent.click(screen.getByTestId("settings-help-ask-next"));

    expect(screen.getByRole("radio", { name: "Aki" })).toBeInTheDocument();
    expect(screen.queryByRole("radio", { name: "Old app" })).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-007
  it("no one in the room can be asked, the dialog says so and cannot send", () => {
    mount([peer("old-id", "Old app", [])]);

    fireEvent.click(screen.getByRole("button", { name: "ask for help" }));
    fireEvent.click(screen.getByTestId("settings-help-ask-next"));

    expect(screen.getByTestId("settings-help-ask-none")).toHaveTextContent(en.settingsHelp.ask.none);
    fireEvent.click(screen.getByTestId("settings-help-ask-send"));
    expect(calls).toEqual([]);
  });

  // Verifies: REQ-RMT-001
  it("the person picks someone and asks, the ask goes to that person and the bar says they are waiting", async () => {
    mount([AKI, BO]);

    await askFor("Bo");

    expect(calls).toContainEqual({ command: "settings_help_ask", args: { connId: CONN, peerId: "bo-id" } });
    expect(screen.getByTestId("settings-help-bar")).toHaveTextContent(fill(en.settingsHelp.ask.waiting, { name: "Bo" }));
    expect(screen.queryByTestId("settings-help-ask")).not.toBeInTheDocument();
  });

  it("two people to ask and none picked, asking sends nothing", () => {
    mount([AKI, BO]);

    fireEvent.click(screen.getByRole("button", { name: "ask for help" }));
    fireEvent.click(screen.getByTestId("settings-help-ask-next"));
    fireEvent.click(screen.getByTestId("settings-help-ask-send"));

    expect(calls).toEqual([]);
  });

  it("the person backs out, nothing is sent", () => {
    mount([AKI]);

    fireEvent.click(screen.getByRole("button", { name: "ask for help" }));
    fireEvent.click(screen.getByTestId("settings-help-ask-cancel"));

    expect(screen.queryByTestId("settings-help-ask")).not.toBeInTheDocument();
    expect(calls).toEqual([]);
  });

  // Verifies: REQ-RMT-005
  it("while this app waits for an answer, it cannot ask again", async () => {
    mount([AKI, BO]);
    await askFor("Aki");

    expect(screen.queryByRole("button", { name: "ask for help" })).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-003
  it("the person withdraws the ask, it is stopped and they can ask again", async () => {
    mount([AKI]);
    await askFor("Aki");

    fireEvent.click(screen.getByTestId("settings-help-cancel"));

    expect(calls).toContainEqual({ command: "settings_help_stop", args: { connId: CONN, role: "helped" } });
    expect(screen.getByRole("button", { name: "ask for help" })).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-001
  it("the other person says not now, the wait ends and says so", async () => {
    const { send } = mount([AKI]);
    await askFor("Aki");

    send({ type: "declined", role: "helped", peer: "aki-id", busy: false });

    expect(screen.getByText(fill(en.settingsHelp.ask.declined, { name: "Aki" }))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "ask for help" })).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-005
  it("the other person is already helping someone, the wait ends and says so", async () => {
    const { send } = mount([AKI]);
    await askFor("Aki");

    send({ type: "declined", role: "helped", peer: "aki-id", busy: true });

    expect(screen.getByText(fill(en.settingsHelp.ask.busy, { name: "Aki" }))).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-001
  it("the other person says yes, the help is started from here without asking again", async () => {
    const { send } = mount([AKI]);
    await askFor("Aki");

    send({ type: "taken", peer: "aki-id", peer_name: "Aki" });

    expect(calls).toContainEqual({ command: "settings_help_begin", args: { connId: CONN, peerId: "aki-id" } });
    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
    send({ type: "started", role: "helped", peer: "aki-id", peer_name: "Aki" });
    expect(screen.getByTestId("settings-help-bar")).toHaveTextContent(fill(en.settingsHelp.helped.bar, { name: "Aki" }));
  });

  // Verifies: REQ-RMT-001
  it("the help cannot be started here, the wait is not left up and it says so", async () => {
    invoke.mockImplementation(async (command: string, args?: unknown) => {
      calls.push({ command, args });
      if (command === "settings_help_begin") throw new Error("could not reach the relay");
      return undefined;
    });
    const { send } = mount([AKI]);
    await askFor("Aki");

    send({ type: "taken", peer: "aki-id", peer_name: "Aki" });

    await waitFor(() =>
      expect(screen.getByText(fill(en.settingsHelp.ask.couldNotStart, { name: "Aki" }))).toBeInTheDocument()
    );
    expect(screen.queryByTestId("settings-help-bar")).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-003
  it("no one answers in time, the ask is let go and says so", async () => {
    vi.useFakeTimers();
    try {
      const { send } = mount([AKI]);
      fireEvent.click(screen.getByRole("button", { name: "ask for help" }));
      fireEvent.click(screen.getByTestId("settings-help-ask-next"));
      fireEvent.click(screen.getByTestId("settings-help-ask-send"));
      await act(async () => {
        await Promise.resolve();
      });
      expect(screen.getByTestId("settings-help-bar")).toBeInTheDocument();
      void send;

      await act(async () => {
        vi.advanceTimersByTime(ASK_WAIT_MS + 1);
      });

      expect(calls).toContainEqual({ command: "settings_help_stop", args: { connId: CONN, role: "helped" } });
      expect(screen.getByText(fill(en.settingsHelp.ask.noAnswer, { name: "Aki" }))).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("being asked", () => {
  // Verifies: REQ-RMT-001
  it("someone asks for help, the question names them and helping answers yes to that person", async () => {
    const { send } = mount([AKI]);

    send({ type: "asked", peer: "aki-id", peer_name: "Aki" });
    expect(screen.getByTestId("settings-help-question")).toHaveTextContent(
      fill(en.settingsHelp.request.message, { name: "Aki" })
    );
    await untilAllowIsReady();
    fireEvent.click(screen.getByTestId("settings-help-allow"));

    expect(calls).toContainEqual({
      command: "settings_help_answer",
      args: { connId: CONN, peerId: "aki-id", accept: true },
    });
  });

  // Verifies: REQ-RMT-001
  it("someone asks for help and the user says not now, the answer is no and the question closes", () => {
    const { send } = mount([AKI]);

    send({ type: "asked", peer: "aki-id", peer_name: "Aki" });
    fireEvent.click(screen.getByTestId("settings-help-decline"));

    expect(calls).toContainEqual({
      command: "settings_help_answer",
      args: { connId: CONN, peerId: "aki-id", accept: false },
    });
    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-001
  it("helping closes the question at once and asks nothing again", async () => {
    const { send } = mount([AKI]);
    send({ type: "asked", peer: "aki-id", peer_name: "Aki" });
    await untilAllowIsReady();

    fireEvent.click(screen.getByTestId("settings-help-allow"));

    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
    send({ type: "started", role: "helper", peer: "aki-id", peer_name: "Aki" });
    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
    expect(calls.filter((c) => c.command === "settings_help_answer")).toHaveLength(1);
  });

  // Verifies: REQ-RMT-001
  it("answering yes fails, the question is not left waiting for a help that will not begin", async () => {
    invoke.mockImplementation(async (command: string, args?: unknown) => {
      calls.push({ command, args });
      if (command === "settings_help_answer") throw new Error("not asked");
      return undefined;
    });
    const { send } = mount([AKI]);
    send({ type: "asked", peer: "aki-id", peer_name: "Aki" });
    await untilAllowIsReady();

    fireEvent.click(screen.getByTestId("settings-help-allow"));

    await waitFor(() => expect(calls.some((c) => c.command === "settings_help_answer")).toBe(true));
    expect(screen.queryByTestId("settings-help-bar")).not.toBeInTheDocument();
    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-002
  it("a question that has just appeared does not take a click on yes, so a double click answers only the first", () => {
    const { send } = mount([AKI]);
    send({ type: "asked", peer: "aki-id", peer_name: "Aki" });

    fireEvent.click(screen.getByTestId("settings-help-allow"));

    expect(calls.filter((c) => c.command === "settings_help_answer")).toEqual([]);
    expect(screen.getByTestId("settings-help-allow")).toHaveAttribute("aria-disabled", "true");
  });

  // Verifies: REQ-RMT-003
  it("an ask and its withdrawal arrive in the same poll, no question is left up and it says so", () => {
    const { sendAll } = mount([AKI]);

    sendAll([
      { type: "asked", peer: "aki-id", peer_name: "Aki" },
      { type: "ended", role: "helper", peer: "aki-id", reason: "peer_stopped" },
    ]);

    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
    expect(screen.getByText(fill(en.settingsHelp.ask.withdrawn, { name: "Aki" }))).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-002
  it("the same person withdraws and asks again in one poll, the new question holds yes back again", async () => {
    const { send, sendAll } = mount([AKI]);
    send({ type: "asked", peer: "aki-id", peer_name: "Aki" });
    await untilAllowIsReady();

    sendAll([
      { type: "ended", role: "helper", peer: "aki-id", reason: "peer_stopped" },
      { type: "asked", peer: "aki-id", peer_name: "Aki" },
    ]);

    expect(screen.getByTestId("settings-help-allow")).toHaveAttribute("aria-disabled", "true");
  });

  it("the question closes, focus goes back where it was", () => {
    const { send } = mount([AKI]);
    const before = document.createElement("button");
    document.body.appendChild(before);
    before.focus();

    send({ type: "asked", peer: "aki-id", peer_name: "Aki" });
    expect(screen.getByTestId("settings-help-decline")).toHaveFocus();
    fireEvent.click(screen.getByTestId("settings-help-decline"));

    expect(before).toHaveFocus();
    before.remove();
  });

  // Verifies: REQ-RMT-003
  it("while helped, the bar says who helps and Stop ends it", async () => {
    const { send } = mount([AKI]);
    await askFor("Aki");
    send({ type: "started", role: "helped", peer: "aki-id", peer_name: "Aki" });

    expect(screen.getByTestId("settings-help-bar")).toHaveTextContent(fill(en.settingsHelp.helped.bar, { name: "Aki" }));
    fireEvent.click(screen.getByTestId("settings-help-stop-helped"));

    expect(calls).toContainEqual({ command: "settings_help_stop", args: { connId: CONN, role: "helped" } });
    expect(screen.queryByTestId("settings-help-bar")).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-003
  it("the helper leaves the room, the bar goes and says so", () => {
    const { send } = mount([AKI]);
    send({ type: "started", role: "helped", peer: "aki-id", peer_name: "Aki" });

    send({ type: "ended", role: "helped", peer: "aki-id", reason: "peer_left" });

    expect(screen.queryByTestId("settings-help-bar")).not.toBeInTheDocument();
    expect(screen.getByText(fill(en.settingsHelp.ended.left, { name: "Aki" }))).toBeInTheDocument();
  });
});

describe("helping", () => {
  // Verifies: REQ-RMT-003
  it("the help starts, the bar says who is helped and Stop ends it", () => {
    const { send } = mount([AKI]);
    send({ type: "asked", peer: "aki-id", peer_name: "Aki" });
    send({ type: "started", role: "helper", peer: "aki-id", peer_name: "Aki" });

    expect(screen.getByTestId("settings-help-bar")).toHaveTextContent(fill(en.settingsHelp.helper.bar, { name: "Aki" }));
    fireEvent.click(screen.getByTestId("settings-help-stop-helper"));
    expect(calls).toContainEqual({ command: "settings_help_stop", args: { connId: CONN, role: "helper" } });
    expect(screen.queryByTestId("settings-help-bar")).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-003
  it("the other person stops the help, the bar goes and says so", () => {
    const { send } = mount([AKI]);
    send({ type: "started", role: "helper", peer: "aki-id", peer_name: "Aki" });

    send({ type: "ended", role: "helper", peer: "aki-id", reason: "peer_stopped" });

    expect(screen.queryByTestId("settings-help-bar")).not.toBeInTheDocument();
    expect(screen.getByText(fill(en.settingsHelp.ended.stopped, { name: "Aki" }))).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-003
  it("helping one person while another helps this app, the end of one leaves the other", () => {
    const { send } = mount([AKI, BO]);
    send({ type: "started", role: "helper", peer: "aki-id", peer_name: "Aki" });
    send({ type: "started", role: "helped", peer: "bo-id", peer_name: "Bo" });

    send({ type: "ended", role: "helper", peer: "aki-id", reason: "peer_stopped" });

    expect(screen.getByTestId("settings-help-bar")).toHaveTextContent(fill(en.settingsHelp.helped.bar, { name: "Bo" }));
    expect(screen.queryByTestId("settings-help-stop-helper")).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-003
  it("an end about someone else leaves the help with the person helped now", () => {
    const { send } = mount([AKI, BO]);
    send({ type: "started", role: "helper", peer: "aki-id", peer_name: "Aki" });
    fireEvent.click(screen.getByTestId("settings-help-stop-helper"));
    send({ type: "started", role: "helper", peer: "bo-id", peer_name: "Bo" });

    send({ type: "ended", role: "helper", peer: "aki-id", reason: "peer_left" });

    expect(screen.getByTestId("settings-help-bar")).toHaveTextContent(fill(en.settingsHelp.helper.bar, { name: "Bo" }));
  });

  // Verifies: REQ-RMT-003
  it("the person helped leaves, the notice names them though they are gone from the list", () => {
    let people = [AKI];
    const { send, rerender } = mountWith(() => people);
    send({ type: "started", role: "helper", peer: "aki-id", peer_name: "Aki" });

    people = [];
    rerender();
    send({ type: "ended", role: "helper", peer: "aki-id", reason: "peer_left" });

    expect(screen.getByText(fill(en.settingsHelp.ended.left, { name: "Aki" }))).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-001
  it("the other app could not start the help this app agreed to, the help is not left up and it says so", () => {
    const { send } = mount([AKI]);
    send({ type: "asked", peer: "aki-id", peer_name: "Aki" });

    send({ type: "declined", role: "helper", peer: "aki-id", busy: false });

    expect(screen.getByText(fill(en.settingsHelp.ask.couldNotStart, { name: "Aki" }))).toBeInTheDocument();
  });
});

describe("the chat line for a change", () => {
  // Verifies: REQ-RMT-004
  it.each([
    ["en", en],
    ["ja", ja],
  ])("a helper changed a setting, the line names both people and the setting in %s", async (language, bundle) => {
    await i18n.changeLanguage(language);

    render(
      <ChatMessage
        type="system"
        content=""
        timestamp={0}
        senderName="Bo"
        helperName="Aki"
        systemKind="settings_help_changed"
        setting="input_device"
      />
    );

    expect(
      screen.getByText(
        fill(bundle.chat.system.settingsHelpChanged, {
          helper: "Aki",
          helped: "Bo",
          setting: bundle.settings.devices.inputDevice,
        })
      )
    ).toBeInTheDocument();
  });
});
