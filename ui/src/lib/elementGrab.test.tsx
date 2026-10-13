import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { installElementGrab } from "./elementGrab";

vi.mock("./tauri", () => ({
  appEnvironment: vi.fn(async () => ({
    app_version: "0.1.0-107",
    build: "beta",
    os: "macos",
    os_version: "15",
    arch: "aarch64",
    ram_gb: 32,
    audio_host: "coreaudio",
  })),
  settingsGet: vi.fn(async () => {
    throw new Error("no backend");
  }),
  sessionGet: vi.fn(async () => {
    throw new Error("no backend");
  }),
}));

const OVERLAY = "jamuru-element-grab-overlay";

let stop: () => void;
let written: string[];

beforeEach(() => {
  written = [];
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: vi.fn(async (text: string) => void written.push(text)) },
  });
  Object.defineProperty(window, "isSecureContext", { configurable: true, value: true });
  // jsdom has no layout, so the element under the pointer is given directly.
  document.elementFromPoint = vi.fn(() => document.querySelector("button"));
  stop = installElementGrab();
});

afterEach(() => {
  stop();
  document.getElementById("jamuru-element-grab-toast")?.remove();
});

function press(extra: KeyboardEventInit) {
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Shift", bubbles: true, ...extra }));
}

function release() {
  window.dispatchEvent(new KeyboardEvent("keyup", { key: "Shift", bubbles: true }));
}

function pointAtButton() {
  document.dispatchEvent(new MouseEvent("mousemove", { clientX: 5, clientY: 5, bubbles: true }));
}

describe("element grab", () => {
  // Given Alt+Shift held over a button
  // When it is clicked
  // Then the element's report is on the clipboard and the button's own click did not run
  //
  // Verifies: REQ-RMT-033
  it("copies the element and swallows the click, when Alt and Shift are held", async () => {
    const onClick = vi.fn();
    render(<button data-testid="go" onClick={onClick}>Go</button>);

    press({ altKey: true, shiftKey: true });
    pointAtButton();
    expect(document.getElementById(OVERLAY)).not.toBeNull();
    document.querySelector("button")!.click();

    await waitFor(() => expect(written).toHaveLength(1));
    expect(written[0]).toContain("data-testid: go");
    expect(written[0]).toContain("app version: 0.1.0-107");
    expect(written[0]).toContain("## Audio settings\nunavailable");
    expect(document.getElementById("jamuru-element-grab-toast")?.textContent).toBe(
      "Copied the debug info"
    );
    expect(onClick).not.toHaveBeenCalled();
  });

  // Given only Shift held
  // When a button is clicked
  // Then nothing is highlighted or copied and the button works as usual
  //
  // Verifies: REQ-RMT-033
  it("does nothing, when only Shift is held", () => {
    const onClick = vi.fn();
    render(<button onClick={onClick}>Go</button>);

    press({ shiftKey: true });
    pointAtButton();
    document.querySelector("button")!.click();

    expect(document.getElementById(OVERLAY)).toBeNull();
    expect(written).toEqual([]);
    expect(onClick).toHaveBeenCalledOnce();
  });

  // Given the shortcut was active
  // When a key is released
  // Then the highlight goes and clicks work as usual again
  //
  // Verifies: REQ-RMT-033
  it("stops and removes the highlight, when Alt or Shift is released", () => {
    const onClick = vi.fn();
    render(<button onClick={onClick}>Go</button>);
    press({ altKey: true, shiftKey: true });
    pointAtButton();

    release();
    document.querySelector("button")!.click();

    expect(document.getElementById(OVERLAY)).toBeNull();
    expect(written).toEqual([]);
    expect(onClick).toHaveBeenCalledOnce();
  });

  // Given the shortcut was pressed before the pointer moved
  // When it is pressed
  // Then the element under the last known pointer position is highlighted at once
  //
  // Verifies: REQ-RMT-033
  it("highlights the element under the last pointer position, when the keys are pressed without moving", () => {
    render(<button>Go</button>);
    pointAtButton();

    press({ altKey: true, shiftKey: true });

    expect(document.getElementById(OVERLAY)).not.toBeNull();
  });

  // Given the highlight is on screen
  // When the report is built
  // Then the page itself carries no mark of it, so what is copied is what the person saw
  //
  // Verifies: REQ-RMT-033
  it("leaves the element's own style alone, when it highlights", () => {
    render(<button>Go</button>);
    press({ altKey: true, shiftKey: true });
    pointAtButton();

    expect(document.querySelector("button")!.getAttribute("style")).toBeNull();
  });
});
