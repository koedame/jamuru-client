/**
 * The buffer one participant's audio is played from, in the participant's
 * dialog (ADR-097): holding it at a length, putting it back to the preset,
 * and asking the participant to buffer the user's audio for longer.
 */

import { beforeAll, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import i18n from "../../i18n";
import en from "../../../locales/en.json";
import type { PeerBuffer } from "../../lib/tauri";
import { PeerBufferSection, type PeerBufferSectionProps } from "./PeerBufferSection";

beforeAll(async () => {
  await i18n.changeLanguage("en");
});

const PRESET: PeerBuffer = { own: { mode: "preset" }, delay_ms10: 53, granted: null };
const TERMS = { max_ms10: 300, tolerance_tenths: 5, auto: true };

function show(props: Partial<PeerBufferSectionProps> = {}) {
  const handlers = {
    onDismissHint: vi.fn(),
    onSetOwn: vi.fn(),
    onRestoreHeld: vi.fn(),
    onAsk: vi.fn(),
    onWithdraw: vi.fn(),
  };
  render(
    <PeerBufferSection
      peerName="Aki"
      buffer={PRESET}
      held={null}
      asked={null}
      canAsk
      showHint={false}
      {...handlers}
      {...props}
    />
  );
  return handlers;
}

function type(testId: string, value: string) {
  fireEvent.change(screen.getByTestId(testId), { target: { value } });
}

describe("the buffer held by the user", () => {
  // Verifies: REQ-GUI-035
  it("a length is typed and Set is pressed, the buffer is held at it in tenths of a millisecond", () => {
    const { onSetOwn } = show();

    type("peer-buffer-own", "12.5");
    fireEvent.click(screen.getByTestId("peer-buffer-own-set"));

    expect(onSetOwn).toHaveBeenCalledWith(125);
  });

  // Verifies: REQ-GUI-035
  it.each(["", "abc", "0", "85.1", "-3"])(
    "the length typed is %j, Set is not offered",
    (typed) => {
      const { onSetOwn } = show();

      type("peer-buffer-own", typed);

      expect(screen.getByTestId("peer-buffer-own-set")).toBeDisabled();
      expect(onSetOwn).not.toHaveBeenCalled();
    }
  );

  // Verifies: REQ-GUI-035
  it("the longest buffer, 85ms, is typed, it is held", () => {
    const { onSetOwn } = show();

    type("peer-buffer-own", "85");
    fireEvent.click(screen.getByTestId("peer-buffer-own-set"));

    expect(onSetOwn).toHaveBeenCalledWith(850);
  });

  // Verifies: REQ-GUI-035
  it("the buffer is held, Back to preset gives the preset's own buffer back", () => {
    const { onSetOwn } = show({ buffer: { own: { mode: "fixed", ms10: 150 }, delay_ms10: 150, granted: null } });

    expect(screen.getByTestId("peer-buffer-own")).toHaveValue("15");
    expect(screen.getByTestId("peer-buffer-source")).toHaveTextContent("held at 15ms");
    fireEvent.click(screen.getByTestId("peer-buffer-own-preset"));

    expect(onSetOwn).toHaveBeenCalledWith(null);
  });

  // Verifies: REQ-GUI-035
  it("the preset's buffer is in use, there is no Back to preset", () => {
    show();

    expect(screen.queryByTestId("peer-buffer-own-preset")).not.toBeInTheDocument();
    expect(screen.getByTestId("peer-buffer-now")).toHaveTextContent("5.3ms");
  });

  // Verifies: REQ-GUI-035
  it("the audio thread has not said what is in use, it says it is measuring", () => {
    show({ buffer: { own: { mode: "preset" }, delay_ms10: 0, granted: null } });

    expect(screen.getByTestId("peer-buffer-now")).toHaveTextContent(en.session.participant.detail.measuring);
  });
});

describe("asking the participant to buffer longer", () => {
  // Verifies: REQ-GUI-035
  it("the longest length and the dropouts allowed are set, the ask carries them in tenths", () => {
    const { onAsk } = show();

    type("peer-buffer-ask-ms", "30");
    type("peer-buffer-ask-tolerance", "0.5");
    fireEvent.click(screen.getByTestId("peer-buffer-ask-send"));

    expect(onAsk).toHaveBeenCalledWith({ max_ms10: 300, tolerance_tenths: 5, auto: true });
  });

  // Verifies: REQ-GUI-035
  it("following the connection is turned off, the ask is for the length itself and takes no dropouts figure", () => {
    const { onAsk } = show();

    fireEvent.click(screen.getByTestId("peer-buffer-ask-auto"));
    type("peer-buffer-ask-ms", "40");
    fireEvent.click(screen.getByTestId("peer-buffer-ask-send"));

    expect(screen.queryByTestId("peer-buffer-ask-tolerance")).not.toBeInTheDocument();
    expect(onAsk).toHaveBeenCalledWith({ max_ms10: 400, tolerance_tenths: 0, auto: false });
  });

  // Verifies: REQ-GUI-035
  it.each([
    ["peer-buffer-ask-ms", "86"],
    ["peer-buffer-ask-ms", ""],
    ["peer-buffer-ask-tolerance", "10.1"],
    ["peer-buffer-ask-tolerance", "-1"],
  ])("%s is %j, the ask is not offered", (testId, typed) => {
    show();

    type(testId, typed);

    expect(screen.getByTestId("peer-buffer-ask-send")).toBeDisabled();
  });

  // Verifies: REQ-GUI-035
  it("their app does not take asks, the section offers only the user's own buffer", () => {
    show({ canAsk: false });

    expect(screen.queryByTestId("peer-buffer-ask")).not.toBeInTheDocument();
    expect(screen.getByTestId("peer-buffer-own")).toBeInTheDocument();
  });

  // Verifies: REQ-GUI-035
  it.each([
    ["sending", { state: "sending" }, "Asking Aki"],
    ["pending", { state: "pending" }, "has not answered yet"],
    [
      "granted",
      { state: "granted", applied: { applied_ms10: 120, rounded: true, at_lowest: false, auto_running: true } },
      "buffered at 12ms (rounded to their packet length, following their connection)",
    ],
  ] as const)("the ask is %s, it says so and can be taken back", (_name, standing, text) => {
    const { onWithdraw } = show({ asked: { terms: TERMS, standing } });

    expect(screen.getByTestId("peer-buffer-standing")).toHaveTextContent(text);
    expect(screen.queryByTestId("peer-buffer-ask-send")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("peer-buffer-withdraw"));

    expect(onWithdraw).toHaveBeenCalled();
  });

  // Verifies: REQ-GUI-035
  it.each([
    ["declined", "Aki said no"],
    ["declined_by_setting", "do not take asks"],
    ["restored", "put the buffer back"],
    ["unreachable", "No answer from Aki"],
  ] as const)("the ask ended as %s, it says so and the user can ask again", (state, text) => {
    const { onAsk } = show({ asked: { terms: TERMS, standing: { state } } });

    expect(screen.getByTestId("peer-buffer-standing")).toHaveTextContent(text);
    fireEvent.click(screen.getByTestId("peer-buffer-ask-send"));

    expect(onAsk).toHaveBeenCalled();
  });
});

describe("the buffer held because the participant asked", () => {
  // Verifies: REQ-GUI-036
  it("the user took up an ask, the dialog says how far it is and puts the buffer back on request", () => {
    const { onRestoreHeld } = show({
      buffer: {
        own: { mode: "preset" },
        delay_ms10: 120,
        granted: { applied_ms10: 120, rounded: false, at_lowest: false, auto_running: true },
      },
      held: { terms: TERMS },
    });

    expect(screen.getByTestId("peer-buffer-raised-by")).toHaveTextContent("Aki's ask");
    expect(screen.getByTestId("peer-buffer-held")).toHaveTextContent("up to 30ms");
    expect(screen.getByTestId("peer-buffer-held")).toHaveTextContent("12ms now");
    fireEvent.click(screen.getByTestId("peer-buffer-restore"));

    expect(onRestoreHeld).toHaveBeenCalled();
  });

  // Verifies: REQ-GUI-036
  it("nothing was asked, there is no Put it back", () => {
    show();

    expect(screen.queryByTestId("peer-buffer-held")).not.toBeInTheDocument();
    expect(screen.queryByTestId("peer-buffer-raised-by")).not.toBeInTheDocument();
  });
});

describe("the first-use explanation", () => {
  // Verifies: REQ-GUI-035
  it("it is shown, it can be dismissed", () => {
    const { onDismissHint } = show({ showHint: true });

    expect(screen.getByTestId("peer-buffer-hint")).toHaveTextContent("ask Aki to buffer your audio for longer");
    fireEvent.click(screen.getByTestId("peer-buffer-hint-dismiss"));

    expect(onDismissHint).toHaveBeenCalled();
  });

  // Verifies: REQ-GUI-035
  it("it is not asked for, nothing is shown", () => {
    show({ showHint: false });

    expect(screen.queryByTestId("peer-buffer-hint")).not.toBeInTheDocument();
  });
});
