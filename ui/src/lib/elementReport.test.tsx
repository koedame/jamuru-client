import { afterEach, describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { cssPath } from "./domTarget";
import { buildElementReport } from "./elementReport";
import type { AppEnvironment, AudioSettings, SessionSnapshot } from "./tauri";

const app: AppEnvironment = {
  app_version: "0.1.0-107",
  build: "beta",
  os: "windows",
  os_version: "11",
  arch: "x86_64",
  ram_gb: 16,
  audio_host: "wasapi",
};

const audio = {
  input_devices: [{ id: "in-1", name: "USB Mic", is_bluetooth: false }],
  output_devices: [{ id: "out-1", name: "Buds", is_bluetooth: true }],
  input_device_id: "in-1",
  output_device_id: "out-1",
  input_channel_count: 2,
  output_channel_count: 2,
  input_channels: { left: 1, right: null },
  output_channels: { left: 1, right: 2 },
  transmit_channels: 1,
  buffer_size: 64,
  sample_rate: 48000,
} as unknown as AudioSettings;

const session = {
  phase: "connected",
  error: null,
  room: { participants: [{ id: "a" }, { id: "b" }] },
  streaming_peer_ids: ["b"],
  signaling_reconnect: "idle",
} as unknown as SessionSnapshot;

function Panel({ muted, name }: { muted: boolean; name: string }) {
  return (
    <section className="mixer-panel" data-testid="mixer-panel" data-state={muted ? "muted" : "live"}>
      <button className="mute-button" aria-pressed={muted} disabled={muted} data-testid="mute-button">
        {name}
      </button>
      <input aria-label="Name" defaultValue="alice" />
      <input aria-label="Secret" type="password" defaultValue="hunter2" />
    </section>
  );
}

afterEach(() => {
  window.location.hash = "";
  document.title = "";
});

describe("buildElementReport", () => {
  // Given a button the person pointed at, in a panel that is muted
  // When the report is built
  // Then it names the element, its state, and where to find the code
  //
  // Verifies: REQ-RMT-033
  it("names the element, its state and the source hints, when the element is a button in a muted panel", () => {
    const { getByTestId } = render(<Panel muted name="Bob" />);

    const report = buildElementReport(getByTestId("mute-button"), { windowLabel: "main" });

    expect(report).toContain('data-testid: mute-button');
    expect(report).toContain('text: "Bob"');
    expect(report).toContain("disabled: true");
    expect(report).toContain("aria-pressed: true");
    expect(report).toContain('section.mixer-panel[data-testid="mixer-panel"][data-state="muted"]');
    expect(report).toContain("window: main");
    expect(report).toContain("screen: main");
    expect(report).toMatch(/Panel muted=true name="Bob"/);
    expect(report).toContain('<button class="mute-button"');
  });

  // Given the same panel in another state
  // When the report is built
  // Then it reports that state, not a fixed one
  //
  // Verifies: REQ-RMT-033
  it("reports the state the element is in now, when the panel is live", () => {
    const { getByTestId } = render(<Panel muted={false} name="Bob" />);

    const report = buildElementReport(getByTestId("mute-button"));

    expect(report).toContain("disabled: false");
    expect(report).toContain("aria-pressed: false");
    expect(report).toContain('[data-state="live"]');
  });

  // Given a text field and a password field
  // When the report is built for each
  // Then the text is in the report and the password is not
  //
  // Verifies: REQ-RMT-033
  it("leaves a password out, when the element is a password field", () => {
    const { getByLabelText } = render(<Panel muted={false} name="Bob" />);

    expect(buildElementReport(getByLabelText("Name"))).toContain('value: "alice"');
    const secret = buildElementReport(getByLabelText("Secret"));
    expect(secret).not.toContain("hunter2");
    expect(secret).not.toContain("value:");
  });

  // Given the window someone helping works in
  // When the report is built
  // Then it says so, because the same component is drawn there from another app's state
  //
  // Verifies: REQ-RMT-033
  it("says the screen is the help window, when the hash is the help hash", () => {
    window.location.hash = "#/help";
    document.title = "Helping";
    const { getByTestId } = render(<Panel muted={false} name="Bob" />);

    const report = buildElementReport(getByTestId("mute-button"));

    expect(report).toContain("screen: help");
    expect(report).toContain("page title: Helping");
  });

  // Given an element with a very long text
  // When the report is built
  // Then the text and the html are cut, so one node cannot flood the clipboard
  //
  // Verifies: REQ-RMT-033
  it("cuts a long text and html, when the element holds thousands of characters", () => {
    const { container } = render(<p>{"x".repeat(5000)}</p>);

    const report = buildElementReport(container.querySelector("p")!);

    expect(report.length).toBeLessThan(2500);
    expect(report).toContain("5000 chars");
  });
});

describe("cssPath", () => {
  // Given an element inside a part with a test id
  // When its selector is built
  // Then it starts from the test id and counts same-named siblings
  //
  // Verifies: REQ-RMT-033
  it("starts from the nearest data-testid and numbers same-tag siblings, when the element is the second input", () => {
    const { getAllByRole } = render(<Panel muted={false} name="Bob" />);

    const second = getAllByRole("textbox")[0]!;
    expect(cssPath(second)).toBe('section[data-testid="mixer-panel"] > input:nth-of-type(1)');
  });
});

describe("buildElementReport of a part holding a password field", () => {
  // Given a panel that contains a password field
  // When the report is built for the panel
  // Then the html in it does not carry the password
  //
  // Verifies: REQ-RMT-033
  it("leaves the password out of the html, when the element contains a password field", () => {
    const { getByTestId } = render(<Panel muted={false} name="Bob" />);

    expect(buildElementReport(getByTestId("mixer-panel"))).not.toContain("hunter2");
  });
});

describe("buildElementReport of the app and its settings", () => {
  // Given what the backend says about the app, its audio settings and the session
  // When the report is built
  // Then the version, platform, devices, buffer and room state are in it, and no device id is
  //
  // Verifies: REQ-RMT-033
  it("names the version, platform, devices, audio settings and room state, when the backend answered", () => {
    const { getByTestId } = render(<Panel muted={false} name="Bob" />);

    const report = buildElementReport(getByTestId("mute-button"), { app, audio, session });

    expect(report).toContain("app version: 0.1.0-107");
    expect(report).toContain("build: beta");
    expect(report).toContain("os: windows 11 (x86_64)");
    expect(report).toContain("ram: 16 GB");
    expect(report).toContain("audio host: wasapi");
    expect(report).toContain("input device: USB Mic");
    expect(report).toContain("input channels: 1 (mono) of 2");
    expect(report).toContain("output device: Buds (bluetooth)");
    expect(report).toContain("output channels: 1/2 of 2");
    expect(report).toContain("buffer size: 64 frames");
    expect(report).toContain("sample rate: 48000 Hz");
    expect(report).toContain("phase: connected");
    expect(report).toContain("participants: 2");
    expect(report).toContain("audio links: 1");
    expect(report).not.toContain("in-1");
  });

  // Given a window that could not read the backend
  // When the report is built
  // Then it says each part is unavailable instead of leaving it out silently
  //
  // Verifies: REQ-RMT-033
  it("says the parts are unavailable, when the backend could not be read", () => {
    const { getByTestId } = render(<Panel muted={false} name="Bob" />);

    const report = buildElementReport(getByTestId("mute-button"), { app: null, audio: null, session: null });

    expect(report).toContain("## App\nunavailable in this window");
    expect(report).toContain("## Audio settings\nunavailable");
    expect(report).toContain("## Session\nunavailable");
  });

  // Given the default devices and a session without a room
  // When the report is built
  // Then the default is named as the system default and no participants are counted
  //
  // Verifies: REQ-RMT-033
  it("names the system default device and no room, when nothing is chosen and no room is joined", () => {
    const { getByTestId } = render(<Panel muted={false} name="Bob" />);

    const report = buildElementReport(getByTestId("mute-button"), {
      audio: { ...audio, input_device_id: null } as AudioSettings,
      session: { ...session, room: null, streaming_peer_ids: [] } as unknown as SessionSnapshot,
    });

    expect(report).toContain("input device: system default");
    expect(report).toContain("in a room: false");
    expect(report).not.toContain("participants:");
  });
});
