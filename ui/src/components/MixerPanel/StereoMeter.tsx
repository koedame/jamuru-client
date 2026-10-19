/**
 * StereoMeter - Stereo level meter with peak hold
 *
 * Displays L/R audio levels with a fixed green->yellow->red gradient (ui.pen
 * Molecules/Meter/Stereo) revealed from the bottom, plus a decaying peak marker.
 */

import { usePeakHold } from "./usePeakHold";
import { clampPercent } from "./mixerScale";
import "./StereoMeter.css";

export interface StereoMeterProps {
  /** Left channel level (0-100) */
  levelL: number;
  /** Right channel level (0-100) */
  levelR: number;
  /** Height in pixels. Omit to size by CSS (200px, or filling a flex parent). */
  height?: number;
  /** Whether the channel is muted */
  isMuted?: boolean;
  /** Peak release time in ms */
  peakReleaseTime?: number;
}

export function StereoMeter({
  levelL,
  levelR,
  height,
  isMuted = false,
  peakReleaseTime = 1500,
}: StereoMeterProps) {
  // Shared with ChannelStrip/MasterSection's numeric peak-dB read-outs, so
  // the marker on this bar and any dB number derived from it always agree.
  const peaks = usePeakHold(levelL, levelR, { releaseTime: peakReleaseTime });

  const percentL = clampPercent(levelL);
  const percentR = clampPercent(levelR);
  const peakPercentL = clampPercent(peaks.peakL);
  const peakPercentR = clampPercent(peaks.peakR);

  return (
    <div
      className={`stereo-meter ${isMuted ? "stereo-meter--muted" : ""}`}
      style={height === undefined ? undefined : { height: `${height}px` }}
    >
      <div className="stereo-meter__box">
        <div className="stereo-meter__bar">
          <div className="stereo-meter__fill" />
          <div className="stereo-meter__mask" style={{ height: `${100 - percentL}%` }} />
          <div className="stereo-meter__peak" style={{ bottom: `${peakPercentL}%` }} />
        </div>
        <div className="stereo-meter__bar">
          <div className="stereo-meter__fill" />
          <div className="stereo-meter__mask" style={{ height: `${100 - percentR}%` }} />
          <div className="stereo-meter__peak" style={{ bottom: `${peakPercentR}%` }} />
        </div>
        <div className="stereo-meter__zero" />
      </div>
    </div>
  );
}

export default StereoMeter;
