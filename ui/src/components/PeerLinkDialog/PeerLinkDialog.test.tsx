/**
 * The participant's dialog: the receive buffer of that participant can be
 * held at a length from it (ADR-097), the first-use explanation is shown once,
 * and Tab stays among the dialog's controls.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";

import i18n from "../../i18n";
import type { PeerLinkStatus } from "../../lib/tauri";
import type { BufferRequests } from "../BufferRequest";
import { HINT_MS } from "../BufferRequest";
import { PeerLinkDialog } from "./PeerLinkDialog";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const LINK: PeerLinkStatus = {
  peer_id: "aki-id",
  connection_state: "connected",
  connection_error: null,
  network: null,
  latency: null,
  peer_audio: null,
  level: 0,
  buffer: { own: { mode: "preset" }, delay_ms10: 53, granted: null },
  dropout: null,
  unstable: false,
};

function requests(changes: Partial<BufferRequests> = {}): BufferRequests {
  return {
    askedOf: () => null,
    heldFor: () => null,
    canAsk: true,
    ask: vi.fn(async () => {}),
    withdraw: vi.fn(async () => {}),
    restore: vi.fn(async () => {}),
    toast: null,
    overlays: null,
    ...changes,
  };
}

function open(props: { requests?: BufferRequests; askable?: boolean } = {}) {
  const r = "requests" in props ? props.requests : requests();
  render(
    <PeerLinkDialog
      peerId="aki-id"
      peerName="Aki"
      link={LINK}
      localSampleRate={48000}
      askable={props.askable ?? true}
      requests={r}
      onClose={() => {}}
    />
  );
  return r;
}

beforeEach(async () => {
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
  localStorage.clear();
  await i18n.changeLanguage("en");
});

describe("the buffer of the participant in their dialog", () => {
  // Verifies: REQ-GUI-035
  it("a length is set for the participant, only that participant's buffer is set", () => {
    open();

    fireEvent.change(screen.getByTestId("peer-buffer-own"), { target: { value: "8" } });
    fireEvent.click(screen.getByTestId("peer-buffer-own-set"));

    expect(invoke).toHaveBeenCalledWith("streaming_set_peer_buffer", { peerId: "aki-id", ms10: 80 }, undefined);
  });

  // Verifies: REQ-GUI-035
  it("the user asks, the ask goes out for that participant", () => {
    const r = open();

    fireEvent.click(screen.getByTestId("peer-buffer-ask-send"));

    expect(r!.ask).toHaveBeenCalledWith("aki-id", { max_ms10: 200, tolerance_tenths: 0, auto: true });
  });

  // Verifies: REQ-GUI-035
  it("their app does not take asks, the dialog offers no ask", () => {
    open({ askable: false });

    expect(screen.queryByTestId("peer-buffer-ask")).not.toBeInTheDocument();
  });

  // Verifies: REQ-GUI-035
  it("the dialog belongs to a helper's window, the buffer is not offered", () => {
    open({ requests: undefined });

    expect(screen.queryByTestId("peer-buffer")).not.toBeInTheDocument();
  });

  // Verifies: REQ-GUI-036
  it("the user took up an ask of the participant, Put it back puts it back for that participant", () => {
    const r = open({
      requests: requests({ heldFor: () => ({ terms: { max_ms10: 300, tolerance_tenths: 5, auto: true }, applied: null }) }),
    });

    fireEvent.click(screen.getByTestId("peer-buffer-restore"));

    expect(r!.restore).toHaveBeenCalledWith("aki-id");
  });
});

describe("the first-use explanation", () => {
  // Verifies: REQ-GUI-035
  it("the dialog is opened for the first time, the explanation is shown and goes by itself after twenty seconds", () => {
    vi.useFakeTimers();
    try {
      open();
      expect(screen.getByTestId("peer-buffer-hint")).toBeInTheDocument();

      act(() => vi.advanceTimersByTime(HINT_MS - 1));
      expect(screen.getByTestId("peer-buffer-hint")).toBeInTheDocument();
      act(() => vi.advanceTimersByTime(1));

      expect(screen.queryByTestId("peer-buffer-hint")).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  // Verifies: REQ-GUI-035
  it("the explanation was shown before, a dialog opened later does not show it again", () => {
    const { unmount } = render(
      <PeerLinkDialog peerId="aki-id" peerName="Aki" link={LINK} localSampleRate={48000} askable requests={requests()} onClose={() => {}} />
    );
    expect(screen.getByTestId("peer-buffer-hint")).toBeInTheDocument();
    unmount();

    open();

    expect(screen.queryByTestId("peer-buffer-hint")).not.toBeInTheDocument();
  });

  // Verifies: REQ-GUI-035
  it("the explanation is dismissed, it goes", () => {
    open();

    fireEvent.click(screen.getByTestId("peer-buffer-hint-dismiss"));

    expect(screen.queryByTestId("peer-buffer-hint")).not.toBeInTheDocument();
  });
});

describe("keyboard use of the dialog", () => {
  // Verifies: REQ-GUI-035
  it("Tab is pressed on the last control, focus goes to the first and not out of the dialog", () => {
    open();
    const dialog = screen.getByTestId("peer-link-dialog");
    const controls = Array.from(dialog.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)"));
    const last = controls[controls.length - 1];
    last.focus();

    fireEvent.keyDown(last, { key: "Tab" });

    expect(controls[0]).toHaveFocus();
  });

  // Verifies: REQ-GUI-035
  it("Shift+Tab is pressed on the first control, focus goes to the last", () => {
    open();
    const dialog = screen.getByTestId("peer-link-dialog");
    const controls = Array.from(dialog.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)"));
    controls[0].focus();

    fireEvent.keyDown(controls[0], { key: "Tab", shiftKey: true });

    expect(controls[controls.length - 1]).toHaveFocus();
  });
});
