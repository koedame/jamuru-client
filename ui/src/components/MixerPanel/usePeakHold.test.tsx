/**
 * usePeakHold: the peak marker of a meter holds, then falls.
 *
 * What costs a screen is how often a falling peak is drawn: every animation
 * frame is a layout per channel, for a 2px marker.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { usePeakHold, DECAY_TICK_MS } from './usePeakHold';

let draws = 0;

function useDrawn(levelL: number, levelR: number) {
  draws += 1;
  return usePeakHold(levelL, levelR, { holdTime: 500, releaseTime: 1500 });
}

describe('usePeakHold', () => {
  beforeEach(() => {
    draws = 0;
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('レベルが上がったとき、ピークがそのレベルになること', () => {
    const { result, rerender } = renderHook(({ l, r }) => useDrawn(l, r), {
      initialProps: { l: 0, r: 0 },
    });
    rerender({ l: 70, r: 30 });
    expect(result.current).toEqual({ peakL: 70, peakR: 30 });
  });

  it('レベルが下がったとき、保持時間のあいだはピークが動かないこと', () => {
    const { result, rerender } = renderHook(({ l, r }) => useDrawn(l, r), {
      initialProps: { l: 70, r: 70 },
    });
    rerender({ l: 10, r: 10 });
    act(() => {
      vi.advanceTimersByTime(450);
    });
    expect(result.current).toEqual({ peakL: 70, peakR: 70 });
  });

  it('保持時間を過ぎたとき、解放時間をかけて 0 まで線形に下がること', () => {
    const { result, rerender } = renderHook(({ l, r }) => useDrawn(l, r), {
      initialProps: { l: 60, r: 60 },
    });
    rerender({ l: 0, r: 0 });
    // 保持 500ms のあと解放 1500ms で 100 下がる: 1000ms 経つと 500ms 分 = 33.3 下がる
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(result.current.peakL).toBeCloseTo(60 - (500 * 100) / 1500, 5);
    act(() => {
      vi.advanceTimersByTime(1500);
    });
    expect(result.current).toEqual({ peakL: 0, peakR: 0 });
  });

  it('ピークが下がっているとき、再描画は DECAY_TICK_MS ごとであること', () => {
    const { rerender } = renderHook(({ l, r }) => useDrawn(l, r), {
      initialProps: { l: 100, r: 100 },
    });
    rerender({ l: 0, r: 0 });
    draws = 0;
    // 保持が終わるまで進める
    act(() => {
      vi.advanceTimersByTime(500);
    });
    draws = 0;
    // 画面が 60Hz で進む 1 秒
    for (let frame = 0; frame < 60; frame++) {
      act(() => {
        vi.advanceTimersByTime(1000 / 60);
      });
    }
    expect(draws).toBeLessThanOrEqual(1000 / DECAY_TICK_MS);
    expect(draws).toBeGreaterThan(0);
  });

  it('ピークが 0 のとき、再描画もタイマーも動かないこと', () => {
    renderHook(({ l, r }) => useDrawn(l, r), { initialProps: { l: 0, r: 0 } });
    draws = 0;
    act(() => {
      vi.advanceTimersByTime(5000);
    });
    expect(draws).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('下がりきったとき、タイマーが止まること', () => {
    const { rerender } = renderHook(({ l, r }) => useDrawn(l, r), {
      initialProps: { l: 50, r: 50 },
    });
    rerender({ l: 0, r: 0 });
    act(() => {
      vi.advanceTimersByTime(500 + 1500);
    });
    expect(vi.getTimerCount()).toBe(0);
  });

  it('下がっている途中でレベルがピークを超えたとき、新しいピークから保持し直すこと', () => {
    const { result, rerender } = renderHook(({ l, r }) => useDrawn(l, r), {
      initialProps: { l: 80, r: 80 },
    });
    rerender({ l: 0, r: 0 });
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    rerender({ l: 90, r: 0 });
    expect(result.current.peakL).toBe(90);
    act(() => {
      vi.advanceTimersByTime(450);
    });
    expect(result.current.peakL).toBe(90);
  });

  it('レベルが変わらないまま保持時間を過ぎたとき、ピークがレベルを下回らないこと', () => {
    const { result } = renderHook(() => useDrawn(40, 40));
    act(() => {
      vi.advanceTimersByTime(5000);
    });
    expect(result.current.peakL).toBeGreaterThanOrEqual(40);
  });
});
