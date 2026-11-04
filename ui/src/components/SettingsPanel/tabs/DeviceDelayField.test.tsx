/**
 * DeviceDelayField: the device delay measurement in the Devices tab (REQ-GUI-034)
 */

import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";

import "../../../i18n";
import en from "../../../../locales/en.json";
import ja from "../../../../locales/ja.json";
import type { DeviceDelayStatus } from "../../../lib/tauri";
import { DeviceDelayField, type DeviceDelayProps } from "./DeviceDelayField";

const idle: DeviceDelayStatus = {
  session_running: true,
  running: false,
  stimulus: null,
  step: null,
  steps: 8,
  failure: null,
  figure: null,
  peers: {},
};

function show(status: DeviceDelayStatus | null, over: Partial<DeviceDelayProps> = {}) {
  const props = { status, canStart: true, onStart: vi.fn(), onCancel: vi.fn(), ...over };
  render(<DeviceDelayField {...props} />);
  return props;
}

const figure = (delay_ms: number, over: Partial<NonNullable<DeviceDelayStatus["figure"]>> = {}) => ({
  delay_ms,
  spread_ms: 0.5,
  stability: "steady" as const,
  needs_review: false,
  measured_at: 1,
  ...over,
});

describe("DeviceDelayField", () => {
  // Verifies: REQ-GUI-034
  it("部屋に入っていて何も測っていないとき、測るボタンが押せて、押すと測りが始まること", () => {
    const props = show(idle);

    fireEvent.click(screen.getByTestId("device-delay-start"));

    expect(props.onStart).toHaveBeenCalledTimes(1);
    expect(screen.getByText(en.settings.devices.delay.none)).toBeInTheDocument();
  });

  // Verifies: REQ-GUI-034
  it("部屋に入っていないとき、測るボタンが押せず、部屋に入るよう案内が出ること", () => {
    show({ ...idle, session_running: false });

    expect(screen.getByTestId("device-delay-start")).toBeDisabled();
    expect(screen.getByText(en.settings.devices.delay.needsRoom)).toBeInTheDocument();
  });

  // Verifies: REQ-GUI-034
  it("手伝う側の画面のとき、測るボタンが押せず、測るのは手伝われる人だと案内が出ること", () => {
    show(idle, { canStart: false });

    expect(screen.getByTestId("device-delay-start")).toBeDisabled();
    expect(screen.getByText(en.settings.devices.delay.helperCannotStart)).toBeInTheDocument();
  });

  // Verifies: REQ-GUI-034
  it("測っている最中は、どの音の何段目かが出て、ボタンは止めるに変わること", () => {
    const props = show({ ...idle, running: true, stimulus: "tone", step: 3, steps: 8 });

    expect(screen.getByTestId("device-delay-progress")).toHaveTextContent("a 1 kHz tone");
    expect(screen.getByTestId("device-delay-progress")).toHaveTextContent("3 / 8");
    expect(screen.queryByTestId("device-delay-start")).toBeNull();
    fireEvent.click(screen.getByTestId("device-delay-cancel"));
    expect(props.onCancel).toHaveBeenCalledTimes(1);
  });

  // Verifies: REQ-GUI-034
  it("安定して測れたとき、遅延と安定を出し、見直しの案内は出さないこと", () => {
    show({ ...idle, figure: figure(23.04) });

    expect(screen.getByTestId("device-delay-result")).toHaveTextContent("23.0 ms");
    expect(screen.getByTestId("device-delay-result")).toHaveTextContent("Steady");
    expect(screen.queryByTestId("device-delay-review")).toBeNull();
  });

  // Verifies: REQ-GUI-034
  it("100 ms を超える遅延のとき、機器の設定を見直す案内が出ること", () => {
    show({ ...idle, figure: figure(140, { needs_review: true, stability: "slightly_shaky" }) });

    expect(screen.getByTestId("device-delay-review")).toBeInTheDocument();
    expect(screen.getByTestId("device-delay-result")).toHaveTextContent("Slightly unsteady");
  });

  // Verifies: REQ-GUI-034
  it("測れなかったとき、理由が出ること", () => {
    show({ ...idle, failure: "no_sound" });

    expect(screen.getByTestId("device-delay-failure")).toHaveTextContent("The sound did not come back");
  });

  // Verifies: REQ-GUI-034
  it("使えるすべての失敗の理由と安定の言葉が、日本語にも英語にも用意されていること", () => {
    for (const lang of [ja, en]) {
      const delay = lang.settings.devices.delay;
      expect(Object.keys(delay.failure).sort()).toEqual(["no_sound", "no_stream", "timed_out", "too_long", "unsteady"]);
      expect(Object.keys(delay.stability).sort()).toEqual(["shaky", "slightly_shaky", "steady"]);
    }
  });
});
