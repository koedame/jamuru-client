/**
 * Tests for where AddReactionButton puts its emoji picker: the chat column
 * clips its children, so the picker must sit on the document body and stay
 * inside the window.
 */
import { describe, it, expect, afterEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import { AddReactionButton } from "./AddReactionButton";

const WINDOW_WIDTH = 1134;
const WINDOW_HEIGHT = 668;
const PICKER_WIDTH = 280;
const PICKER_HEIGHT = 128;

function placeTriggerAt(rect: { left: number; top: number; width: number; height: number }) {
  const trigger = screen.getByRole("button");
  vi.spyOn(trigger, "getBoundingClientRect").mockReturnValue({
    ...rect,
    x: rect.left,
    y: rect.top,
    right: rect.left + rect.width,
    bottom: rect.top + rect.height,
    toJSON: () => ({}),
  });
  return trigger;
}

function openPicker(rect: { left: number; top: number; width: number; height: number }) {
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(PICKER_WIDTH);
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(PICKER_HEIGHT);
  const { container } = render(<AddReactionButton onSelect={() => {}} />);
  fireEvent.click(placeTriggerAt(rect));
  const picker = screen.getByRole("dialog").parentElement as HTMLElement;
  return { container, picker };
}

describe("AddReactionButton", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("ピッカーを開いたとき、チャット列に切り取られないよう body 直下に出ること", () => {
    const { container, picker } = openPicker({ left: 871, top: 300, width: 24, height: 24 });

    expect(picker.parentElement).toBe(document.body);
    expect(container.contains(picker)).toBe(false);
  });

  it("ボタンが窓の右端にあるとき、ピッカーの右端が窓からはみ出さないこと", () => {
    const { picker } = openPicker({ left: 871, top: 300, width: 24, height: 24 });

    const left = parseFloat(picker.style.left);
    expect(left + PICKER_WIDTH).toBeLessThanOrEqual(WINDOW_WIDTH);
    expect(left).toBeGreaterThanOrEqual(0);
  });

  it("ボタンの上に余白があるとき、ピッカーがボタンの上に出ること", () => {
    const { picker } = openPicker({ left: 100, top: 300, width: 24, height: 24 });

    expect(parseFloat(picker.style.top) + PICKER_HEIGHT).toBeLessThanOrEqual(300);
  });

  it("ボタンが窓の上端に近いとき、ピッカーがボタンの下に出て窓の中に収まること", () => {
    const { picker } = openPicker({ left: 100, top: 20, width: 24, height: 24 });

    const top = parseFloat(picker.style.top);
    expect(top).toBeGreaterThanOrEqual(44);
    expect(top + PICKER_HEIGHT).toBeLessThanOrEqual(WINDOW_HEIGHT);
  });

  it("絵文字を選んだとき、onSelect に渡されてピッカーが閉じること", () => {
    const onSelect = vi.fn();
    render(<AddReactionButton onSelect={onSelect} />);
    fireEvent.click(screen.getByRole("button"));

    fireEvent.click(screen.getByRole("button", { name: "😀" }));

    expect(onSelect).toHaveBeenCalledWith("😀");
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
