/**
 * The small window a start shows while it updates the app before opening it
 * (ADR-089): it follows how far the update is and passes on the wish to open
 * the app without waiting.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";

import i18n from "../i18n";
import type { StartupProgress } from "../lib/tauri";
import { StartupUpdateScreen } from "./StartupUpdateScreen";

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
let progress: StartupProgress | null;

function announce(payload: StartupProgress) {
  act(() => (listeners.get("update:startup") ?? []).forEach((heard) => heard({ payload })));
}

beforeEach(async () => {
  calls = [];
  listeners.clear();
  progress = { phase: "checking" };
  invoke.mockReset();
  invoke.mockImplementation(async (command: string) => {
    calls.push(command);
    if (command === "update_startup_progress") return progress;
    return undefined;
  });
  await i18n.changeLanguage("en");
});

describe("StartupUpdateScreen", () => {
  // Verifies: REQ-UPD-025
  it("the window opens in the middle of a download, it shows the download as far as it is", async () => {
    progress = { phase: "downloading", received: 50, total: 100 };

    render(<StartupUpdateScreen />);

    expect(await screen.findByText("50%")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Downloading the new version");
  });

  // Verifies: REQ-UPD-025
  it("the backend reports the update moving on, the window follows it", async () => {
    render(<StartupUpdateScreen />);
    await waitFor(() => expect(calls).toContain("update_startup_progress"));

    announce({ phase: "downloading", received: 10, total: 100 });
    expect(screen.getByText("10%")).toBeInTheDocument();

    announce({ phase: "installing" });
    expect(screen.getByRole("status")).toHaveTextContent("Installing the new version");
    expect(screen.queryByTestId("startup-update-skip")).not.toBeInTheDocument();
  });

  // Verifies: REQ-UPD-025
  it("a report heard before the read is answered is not undone by the older answer", async () => {
    let answer: (now: StartupProgress | null) => void = () => {};
    invoke.mockImplementation((command: string) => {
      calls.push(command);
      if (command === "update_startup_progress") return new Promise((resolve) => (answer = resolve));
      return Promise.resolve(undefined);
    });
    render(<StartupUpdateScreen />);
    await waitFor(() => expect(calls).toContain("update_startup_progress"));

    announce({ phase: "downloading", received: 60, total: 100 });
    await act(async () => answer({ phase: "downloading", received: 59, total: 100 }));

    expect(screen.getByText("60%")).toBeInTheDocument();
  });

  // Verifies: REQ-UPD-025
  it("the person presses Open without waiting, the backend is asked once and the button is held", async () => {
    render(<StartupUpdateScreen />);
    const button = await screen.findByTestId("startup-update-skip");

    fireEvent.click(button);

    await waitFor(() => expect(calls.filter((c) => c === "update_startup_skip")).toHaveLength(1));
    expect(button).toBeDisabled();
  });

  // Verifies: REQ-UPD-025
  it("asking the backend fails, the button can be pressed again", async () => {
    invoke.mockImplementation(async (command: string) => {
      calls.push(command);
      if (command === "update_startup_skip") throw new Error("gone");
      if (command === "update_startup_progress") return progress;
      return undefined;
    });
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    render(<StartupUpdateScreen />);
    const button = await screen.findByTestId("startup-update-skip");

    fireEvent.click(button);

    await waitFor(() => expect(button).toBeEnabled());
    error.mockRestore();
  });
});
