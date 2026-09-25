/**
 * Helping with settings in the main window (ADR-043): who is offered help,
 * the questions the helped side answers, the helper's panel, and stopping.
 * The app's side (what is sent, what is applied) is `src-tauri/src/settings_help.rs`.
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { useEffect } from "react";

import i18n from "../../i18n";
import en from "../../../locales/en.json";
import ja from "../../../locales/ja.json";
import type { HelpEvent, PeerInfo } from "../../lib/tauri";
import { ChatMessage } from "../ChatPanel/ChatMessage";
import { audioSettings } from "../SettingsPanel/audioSettingsFixture";
import { ALLOW_DELAY_MS } from "./SettingsHelpQuestion";
import { useSettingsHelp, type SettingsHelp } from "./useSettingsHelp";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

const CONN = 7;

function peer(id: string, name: string, features: string[] = ["peer_message"]): PeerInfo {
  return { id, name, candidates: [], public_addr: null, local_addr: null, features };
}

const AKI = peer("aki-id", "Aki");
const BO = peer("bo-id", "Bo");

/** Mounts the hook as the main window does, and hands back its handle. */
function mount(participants: PeerInfo[]) {
  return mountWith(() => participants);
}

/** `mount`, with the participants read on each render (`rerender` after changing them). */
function mountWith(participantsNow: () => PeerInfo[]) {
  let help: SettingsHelp | undefined;
  function Harness() {
    const participants = participantsNow();
    const h = useSettingsHelp(CONN, participants);
    useEffect(() => {
      help = h;
    });
    return (
      <>
        {participants.map((p) =>
          h.canOffer(p) ? (
            <button key={p.id} onClick={() => h.offer(p.id)}>
              {`offer ${p.name}`}
            </button>
          ) : null
        )}
        {h.bars}
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

beforeEach(async () => {
  calls = [];
  invoke.mockReset();
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    calls.push({ command, args });
    switch (command) {
      case "settings_get":
        return audioSettings({
          input_devices: [
            { id: "alsa:scarlett", name: "Scarlett 2i2", supported_sample_rates: [48000], supported_channels: [2], is_default: false, is_asio: false },
          ],
        });
      case "settings_help_propose":
        return 1;
      default:
        return undefined;
    }
  });
  await i18n.changeLanguage("en");
});

describe("offering help", () => {
  // Verifies: REQ-RMT-007
  it("a participant whose app takes peer messages is offered help, one whose app does not is not", () => {
    mount([AKI, peer("old-id", "Old app", [])]);

    expect(screen.getByRole("button", { name: "offer Aki" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "offer Old app" })).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-005
  it("while this app helps someone, it offers help to no one else", async () => {
    mount([AKI, BO]);

    fireEvent.click(screen.getByRole("button", { name: "offer Aki" }));

    await waitFor(() => expect(screen.queryByRole("button", { name: "offer Bo" })).not.toBeInTheDocument());
    expect(calls).toContainEqual({ command: "settings_help_request", args: { connId: CONN, peerId: "aki-id" } });
    expect(screen.getByTestId("settings-help-bar")).toHaveTextContent(fill(en.settingsHelp.helper.asking, { name: "Aki" }));
  });

  // Verifies: REQ-RMT-001
  it("the other person declines, the offer ends and says so", async () => {
    const { send } = mount([AKI]);
    fireEvent.click(screen.getByRole("button", { name: "offer Aki" }));
    await waitFor(() => expect(screen.getByTestId("settings-help-bar")).toBeInTheDocument());

    send({ type: "declined", peer: "aki-id", busy: false });

    expect(screen.getByText(fill(en.settingsHelp.helper.declined, { name: "Aki" }))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "offer Aki" })).toBeInTheDocument();
  });
});

describe("being helped", () => {
  // Verifies: REQ-RMT-001
  it("someone asks to help, the question names them and allowing answers yes to that person", async () => {
    const { send } = mount([AKI]);

    send({ type: "requested", peer: "aki-id", peer_name: "Aki" });
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
  it("someone asks to help and the user declines, the answer is no and the question closes", () => {
    const { send } = mount([AKI]);

    send({ type: "requested", peer: "aki-id", peer_name: "Aki" });
    fireEvent.click(screen.getByTestId("settings-help-decline"));

    expect(calls).toContainEqual({
      command: "settings_help_answer",
      args: { connId: CONN, peerId: "aki-id", accept: false },
    });
    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-002
  it("the helper proposes a device, the question names the setting and the device, and allowing approves it", async () => {
    const { send } = mount([AKI]);
    send({ type: "requested", peer: "aki-id", peer_name: "Aki" });
    send({ type: "started", role: "helped", peer: "aki-id", settings: null });

    send({ type: "proposed", id: 3, change: { setting: "input_device", device_id: "alsa:scarlett" }, device_name: "Scarlett 2i2" });

    await waitFor(() =>
      expect(screen.getByTestId("settings-help-question")).toHaveTextContent(
        fill(en.settingsHelp.proposal.message, {
          name: "Aki",
          setting: en.settings.devices.inputDevice,
          value: "Scarlett 2i2",
        })
      )
    );
    await untilAllowIsReady();
    fireEvent.click(screen.getByTestId("settings-help-allow"));
    expect(calls).toContainEqual({ command: "settings_help_decide", args: { connId: CONN, id: 3, approve: true } });
  });

  // Verifies: REQ-RMT-002
  it("a question that has just appeared does not take a click on Allow, so a double click answers only the first", async () => {
    const { send } = mount([AKI]);
    send({ type: "requested", peer: "aki-id", peer_name: "Aki" });
    send({ type: "started", role: "helped", peer: "aki-id", settings: null });
    send({ type: "proposed", id: 1, change: { setting: "buffer_size", samples: 128 }, device_name: null });
    await untilAllowIsReady();

    fireEvent.click(screen.getByTestId("settings-help-allow"));
    send({ type: "proposed", id: 2, change: { setting: "transmit_channels", count: 1 }, device_name: null });
    fireEvent.click(screen.getByTestId("settings-help-allow"));

    const decided = calls.filter((c) => c.command === "settings_help_decide");
    expect(decided).toEqual([{ command: "settings_help_decide", args: { connId: CONN, id: 1, approve: true } }]);
    expect(screen.getByTestId("settings-help-question")).toHaveTextContent(en.settings.devices.transmitChannels);
    expect(screen.getByTestId("settings-help-decline")).toHaveFocus();
  });

  // Verifies: REQ-RMT-003
  it("a request and its withdrawal arrive in the same poll, no question is left up", () => {
    const { sendAll } = mount([AKI]);

    sendAll([
      { type: "requested", peer: "aki-id", peer_name: "Aki" },
      { type: "ended", role: "helped", peer: "aki-id", reason: "peer_stopped" },
    ]);

    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
    expect(screen.getByText(fill(en.settingsHelp.ended.stopped, { name: "Aki" }))).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-002
  it("the same person withdraws and asks again in one poll, the new question holds Allow back again", async () => {
    const { send, sendAll } = mount([AKI]);
    send({ type: "requested", peer: "aki-id", peer_name: "Aki" });
    await untilAllowIsReady();

    sendAll([
      { type: "ended", role: "helped", peer: "aki-id", reason: "peer_stopped" },
      { type: "requested", peer: "aki-id", peer_name: "Aki" },
    ]);

    expect(screen.getByTestId("settings-help-allow")).toHaveAttribute("aria-disabled", "true");
  });

  it("the question closes, focus goes back where it was", async () => {
    const { send } = mount([AKI]);
    const before = document.createElement("button");
    document.body.appendChild(before);
    before.focus();

    send({ type: "requested", peer: "aki-id", peer_name: "Aki" });
    expect(screen.getByTestId("settings-help-decline")).toHaveFocus();
    fireEvent.click(screen.getByTestId("settings-help-decline"));

    expect(before).toHaveFocus();
    before.remove();
  });

  // Verifies: REQ-RMT-003
  it("while a change is asked about, the question itself offers to stop the help", () => {
    const { send } = mount([AKI]);
    send({ type: "requested", peer: "aki-id", peer_name: "Aki" });
    send({ type: "started", role: "helped", peer: "aki-id", settings: null });
    send({ type: "proposed", id: 1, change: { setting: "buffer_size", samples: 128 }, device_name: null });

    fireEvent.click(screen.getByTestId("settings-help-question-stop"));

    expect(calls).toContainEqual({ command: "settings_help_stop", args: { connId: CONN, role: "helped" } });
    expect(screen.queryByTestId("settings-help-question")).not.toBeInTheDocument();
    expect(screen.queryByTestId("settings-help-bar")).not.toBeInTheDocument();
  });

  // Verifies: REQ-RMT-006
  it("a proposed device without a name is not named by its id", async () => {
    const { send } = mount([AKI]);
    send({ type: "requested", peer: "aki-id", peer_name: "Aki" });
    send({ type: "started", role: "helped", peer: "aki-id", settings: null });

    send({ type: "proposed", id: 1, change: { setting: "input_device", device_id: "coreaudio:AG06:Y8XJ2KA0123456:1,2" }, device_name: null });

    await waitFor(() =>
      expect(screen.getByTestId("settings-help-question")).toHaveTextContent(en.settingsHelp.value.unknownDevice)
    );
    expect(screen.getByTestId("settings-help-question")).not.toHaveTextContent("Y8XJ2KA0123456");
  });

  // Verifies: REQ-RMT-003
  it("while helped, the bar says who helps and Stop ends it", () => {
    const { send } = mount([AKI]);
    send({ type: "requested", peer: "aki-id", peer_name: "Aki" });
    send({ type: "started", role: "helped", peer: "aki-id", settings: null });

    expect(screen.getByTestId("settings-help-bar")).toHaveTextContent(fill(en.settingsHelp.helped.bar, { name: "Aki" }));
    fireEvent.click(screen.getByTestId("settings-help-stop-helped"));

    expect(calls).toContainEqual({ command: "settings_help_stop", args: { connId: CONN, role: "helped" } });
    expect(screen.queryByTestId("settings-help-bar")).not.toBeInTheDocument();
  });
});

describe("helping", () => {
  // Verifies: REQ-RMT-002
  it("the other person allows the help, their settings open and a choice is proposed and shown as waiting", async () => {
    const { send } = mount([AKI]);
    fireEvent.click(screen.getByRole("button", { name: "offer Aki" }));
    await waitFor(() => expect(screen.getByTestId("settings-help-bar")).toBeInTheDocument());

    send({ type: "started", role: "helper", peer: "aki-id", settings: audioSettings({ buffer_size: 64 }) });
    const panel = await screen.findByTestId("settings-help-panel");
    fireEvent.change(panel.querySelector("#buffer-size")!, { target: { value: "128" } });

    await waitFor(() =>
      expect(calls).toContainEqual({
        command: "settings_help_propose",
        args: { connId: CONN, change: { setting: "buffer_size", samples: 128 } },
      })
    );
    expect(await screen.findByTestId("settings-help-panel-status")).toHaveTextContent(
      fill(en.settingsHelp.helper.pending, { name: "Aki" })
    );
  });

  // Verifies: REQ-RMT-002
  it("an approved change comes back, the panel shows the settings now in effect", async () => {
    const { send } = mount([AKI]);
    fireEvent.click(screen.getByRole("button", { name: "offer Aki" }));
    await waitFor(() => expect(screen.getByTestId("settings-help-bar")).toBeInTheDocument());
    send({ type: "started", role: "helper", peer: "aki-id", settings: audioSettings({ revision: 1, buffer_size: 64 }) });
    const panel = await screen.findByTestId("settings-help-panel");
    fireEvent.change(panel.querySelector("#buffer-size")!, { target: { value: "128" } });
    await waitFor(() => expect(calls.some((c) => c.command === "settings_help_propose")).toBe(true));

    send({ type: "answered", id: 1, answer: { outcome: "applied", settings: audioSettings({ revision: 2, buffer_size: 128 }) } });

    await waitFor(() => expect((panel.querySelector("#buffer-size") as HTMLSelectElement).value).toBe("128"));
    expect(screen.queryByTestId("settings-help-panel-status")).not.toBeInTheDocument();

    // Settings older than those shown arrive late: the panel keeps the newer ones.
    send({ type: "settings", settings: audioSettings({ revision: 1, buffer_size: 64 }) });
    expect((panel.querySelector("#buffer-size") as HTMLSelectElement).value).toBe("128");
  });

  // Verifies: REQ-RMT-002
  it("a change the other app refused says why in the user's language", async () => {
    const { send } = mount([AKI]);
    fireEvent.click(screen.getByRole("button", { name: "offer Aki" }));
    await waitFor(() => expect(screen.getByTestId("settings-help-bar")).toBeInTheDocument());
    send({ type: "started", role: "helper", peer: "aki-id", settings: audioSettings({ buffer_size: 64 }) });
    const panel = await screen.findByTestId("settings-help-panel");
    fireEvent.change(panel.querySelector("#buffer-size")!, { target: { value: "128" } });
    await waitFor(() => expect(calls.some((c) => c.command === "settings_help_propose")).toBe(true));

    send({ type: "answered", id: 1, answer: { outcome: "refused", reason: "device_gone" } });

    expect(await screen.findByTestId("settings-help-panel-status")).toHaveTextContent(
      fill(en.settingsHelp.helper.refused.device_gone, { name: "Aki" })
    );
  });

  // Verifies: REQ-RMT-003
  it("the other person stops the help, the panel closes and says so", async () => {
    const { send } = mount([AKI]);
    fireEvent.click(screen.getByRole("button", { name: "offer Aki" }));
    await waitFor(() => expect(screen.getByTestId("settings-help-bar")).toBeInTheDocument());
    send({ type: "started", role: "helper", peer: "aki-id", settings: audioSettings() });

    send({ type: "ended", role: "helper", peer: "aki-id", reason: "peer_stopped" });

    expect(screen.queryByTestId("settings-help-panel")).not.toBeInTheDocument();
    expect(screen.getByText(fill(en.settingsHelp.ended.stopped, { name: "Aki" }))).toBeInTheDocument();
  });

  // Verifies: REQ-RMT-003
  it("helping one person while another helps this app, the end of one leaves the other", async () => {
    const { send } = mount([AKI, BO]);
    fireEvent.click(screen.getByRole("button", { name: "offer Aki" }));
    await waitFor(() => expect(screen.getByTestId("settings-help-bar")).toBeInTheDocument());
    send({ type: "started", role: "helper", peer: "aki-id", settings: audioSettings() });
    send({ type: "requested", peer: "bo-id", peer_name: "Bo" });
    send({ type: "started", role: "helped", peer: "bo-id", settings: null });

    send({ type: "ended", role: "helper", peer: "aki-id", reason: "peer_stopped" });

    expect(screen.queryByTestId("settings-help-panel")).not.toBeInTheDocument();
    expect(screen.getByTestId("settings-help-bar")).toHaveTextContent(fill(en.settingsHelp.helped.bar, { name: "Bo" }));
  });

  // Verifies: REQ-RMT-003
  it("an end about someone else leaves the offer to the person asked now", async () => {
    const { send } = mount([AKI, BO]);
    fireEvent.click(screen.getByRole("button", { name: "offer Aki" }));
    await waitFor(() => expect(screen.getByTestId("settings-help-bar")).toBeInTheDocument());
    send({ type: "started", role: "helper", peer: "aki-id", settings: audioSettings() });
    fireEvent.click(screen.getByTestId("settings-help-stop-helper"));
    fireEvent.click(screen.getByRole("button", { name: "offer Bo" }));
    await waitFor(() =>
      expect(screen.getByTestId("settings-help-bar")).toHaveTextContent(fill(en.settingsHelp.helper.asking, { name: "Bo" }))
    );

    send({ type: "ended", role: "helper", peer: "aki-id", reason: "peer_left" });

    expect(screen.getByTestId("settings-help-bar")).toHaveTextContent(fill(en.settingsHelp.helper.asking, { name: "Bo" }));
  });

  // Verifies: REQ-RMT-003
  it("the person helped leaves, the notice names them though they are gone from the list", async () => {
    let people = [AKI];
    const { send, rerender } = mountWith(() => people);
    fireEvent.click(screen.getByRole("button", { name: "offer Aki" }));
    await waitFor(() => expect(screen.getByTestId("settings-help-bar")).toBeInTheDocument());
    send({ type: "started", role: "helper", peer: "aki-id", settings: audioSettings() });

    people = [];
    rerender();
    send({ type: "ended", role: "helper", peer: "aki-id", reason: "peer_left" });

    expect(screen.getByText(fill(en.settingsHelp.ended.left, { name: "Aki" }))).toBeInTheDocument();
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
