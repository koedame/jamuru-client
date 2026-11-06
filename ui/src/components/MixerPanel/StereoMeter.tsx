/**
 * StereoMeter - Stereo level meter with peak hold
 *
 * Displays L/R audio levels with a fixed green->yellow->red gradient
 * revealed from the bottom, plus a decaying peak marker.
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

export interface StereoMeterViewProps extends Omit<StereoMeterProps, "peakReleaseTime"> {
  /** Left channel peak (0-100) */
  peakL: number;
  /** Right channel peak (0-100) */
  peakR: number;
}

/** The meter drawn from levels and peaks it is given. A strip that also
 *  shows the peak as a number holds the peaks once and gives them to both. */
export function StereoMeterView({
  levelL,
  levelR,
  peakL,
  peakR,
  height,
  isMuted = false,
}: StereoMeterViewProps) {
  const percentL = clampPercent(levelL);
  const percentR = clampPercent(levelR);
  const peakPercentL = clampPercent(peakL);
  const peakPercentR = clampPercent(peakR);

  return (
    <div
      className={`stereo-meter ${isMuted ? "stereo-meter--muted" : ""}`}
      style={height === undefined ? undefined : { height: `${height}px` }}
    >
      <div className="stereo-meter__box">
        <div className="stereo-meter__bar">
          <div className="stereo-meter__fill" />
          <div className="stereo-meter__mask" style={{ transform: `scaleY(${(100 - percentL) / 100})` }} />
          <div className="stereo-meter__peak" style={{ transform: `translateY(${-peakPercentL}%)` }} />
        </div>
        <div className="stereo-meter__bar">
          <div className="stereo-meter__fill" />
          <div className="stereo-meter__mask" style={{ transform: `scaleY(${(100 - percentR) / 100})` }} />
          <div className="stereo-meter__peak" style={{ transform: `translateY(${-peakPercentR}%)` }} />
        </div>
        <div className="stereo-meter__zero" />
      </div>
    </div>
  );
}

export function StereoMeter({ peakReleaseTime = 1500, ...props }: StereoMeterProps) {
  const peaks = usePeakHold(props.levelL, props.levelR, { releaseTime: peakReleaseTime });
  return <StereoMeterView {...props} peakL={peaks.peakL} peakR={peaks.peakR} />;
}

export default StereoMeter;
