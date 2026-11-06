/**
 * StereoMeter: the bar moves by transform, not by the size of a box.
 *
 * A level changes ten times a second per channel; a changed `height` or
 * `bottom` re-runs layout each time, a changed `transform` does not.
 */

import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import { StereoMeterView } from './StereoMeter';

function parts(container: HTMLElement) {
  return {
    masks: [...container.querySelectorAll<HTMLElement>('.stereo-meter__mask')],
    peaks: [...container.querySelectorAll<HTMLElement>('.stereo-meter__peak')],
  };
}

describe('StereoMeterView', () => {
  it('レベルが 70 と 30 のとき、隠す部分が 0.3 と 0.7 の縦の拡大率になること', () => {
    const { container } = render(<StereoMeterView levelL={70} levelR={30} peakL={0} peakR={0} />);
    const { masks } = parts(container);
    expect(masks[0].style.transform).toBe('scaleY(0.3)');
    expect(masks[1].style.transform).toBe('scaleY(0.7)');
  });

  it('ピークが 80 のとき、目印が棒の高さの 80% だけ持ち上がること', () => {
    const { container } = render(<StereoMeterView levelL={0} levelR={0} peakL={80} peakR={0} />);
    const { peaks } = parts(container);
    expect(peaks[0].style.transform).toBe('translateY(-80%)');
    expect(peaks[1].style.transform).toBe('translateY(0%)');
  });

  it('範囲外の値が来たとき、0 から 100 に丸めて描くこと', () => {
    const { container } = render(<StereoMeterView levelL={150} levelR={-5} peakL={120} peakR={-1} />);
    const { masks, peaks } = parts(container);
    expect(masks[0].style.transform).toBe('scaleY(0)');
    expect(masks[1].style.transform).toBe('scaleY(1)');
    expect(peaks[0].style.transform).toBe('translateY(-100%)');
  });

  it('レベルが変わるとき、レイアウトを起こす height と bottom を書き換えないこと', () => {
    const { container, rerender } = render(<StereoMeterView levelL={10} levelR={10} peakL={10} peakR={10} />);
    rerender(<StereoMeterView levelL={90} levelR={50} peakL={90} peakR={50} />);
    const { masks, peaks } = parts(container);
    for (const el of [...masks, ...peaks]) {
      expect(el.style.height).toBe('');
      expect(el.style.bottom).toBe('');
    }
  });
});
