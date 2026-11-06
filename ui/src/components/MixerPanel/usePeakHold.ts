/**
 * usePeakHold - Hook for tracking peak levels with decay
 *
 * The peak is held for `holdTime`, then falls linearly to 0 over
 * `releaseTime`. Where it is at any moment is worked out from when it was
 * last hit, and the screen is told of it every `DECAY_TICK_MS` only while
 * a peak is above 0: a meter that is drawn on every animation frame costs
 * a layout per channel per frame, for a 2px marker.
 */

import { useCallback, useEffect, useRef, useState } from "react";

interface PeakHoldOptions {
  /** Hold time before decay starts (ms) */
  holdTime?: number;
  /** Time to decay from peak to 0 (ms) */
  releaseTime?: number;
}

interface PeakHoldState {
  peakL: number;
  peakR: number;
}

/** How often a falling peak is redrawn. At the default release time a tick
 *  moves the marker by a third of a percent, under a pixel on a 200px meter. */
export const DECAY_TICK_MS = 50;

interface Hit {
  value: number;
  time: number;
}

interface Hits {
  l: Hit;
  r: Hit;
}

function decayed(hit: Hit, now: number, holdTime: number, releaseTime: number): number {
  const falling = now - hit.time - holdTime;
  if (falling <= 0) return hit.value;
  return Math.max(0, hit.value - (falling * 100) / releaseTime);
}

/** The peaks as of `now`. A level above the fallen peak is the new peak, and
 *  starts a new hold. */
function settle(
  hits: Hits,
  level: { l: number; r: number },
  now: number,
  holdTime: number,
  releaseTime: number
): PeakHoldState {
  const peakL = decayed(hits.l, now, holdTime, releaseTime);
  const peakR = decayed(hits.r, now, holdTime, releaseTime);
  if (level.l > peakL) hits.l = { value: level.l, time: now };
  if (level.r > peakR) hits.r = { value: level.r, time: now };
  return { peakL: Math.max(level.l, peakL), peakR: Math.max(level.r, peakR) };
}

export function usePeakHold(
  levelL: number,
  levelR: number,
  options: PeakHoldOptions = {}
): PeakHoldState {
  const { holdTime = 500, releaseTime = 1500 } = options;

  const [peaks, setPeaks] = useState<PeakHoldState>({ peakL: 0, peakR: 0 });
  const hitsRef = useRef<Hits>({ l: { value: 0, time: 0 }, r: { value: 0, time: 0 } });
  const levelRef = useRef({ l: levelL, r: levelR });

  const update = useCallback(() => {
    const next = settle(hitsRef.current, levelRef.current, Date.now(), holdTime, releaseTime);
    setPeaks((prev) => (prev.peakL === next.peakL && prev.peakR === next.peakR ? prev : next));
  }, [holdTime, releaseTime]);

  // A level above the peak is a new peak
  useEffect(() => {
    levelRef.current = { l: levelL, r: levelR };
    update();
  }, [levelL, levelR, update]);

  // While a peak is up, let it fall
  const falling = peaks.peakL > 0 || peaks.peakR > 0;
  useEffect(() => {
    if (!falling) return;
    const timer = setInterval(update, DECAY_TICK_MS);
    return () => clearInterval(timer);
  }, [falling, update]);

  return peaks;
}

export default usePeakHold;
