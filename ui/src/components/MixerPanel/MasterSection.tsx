/**
 * MasterSection - Horizontal master output meter
 *
 * jamuru brand (ui.pen Molecules/Meter/MasterHorizontal): a display-only master
 * bus meter shown in the room sidebar. Title + dB read-out on top, then L/R
 * horizontal level bars with a 0 dB reference marker. No mute control.
 */

import { useTranslation } from "react-i18next";
import { usePeakHold } from "./usePeakHold";
import { levelToDb, clampPercent } from "./mixerScale";
import "./MasterSection.css";

export interface MasterSectionProps {
  /** Left channel level (0-100) */
  levelL: number;
  /** Right channel level (0-100) */
  levelR: number;
  /** Whether the master output is muted (dims the meters) */
  isMuted?: boolean;
}

export function MasterSection({ levelL, levelR, isMuted = false }: MasterSectionProps) {
  const { t } = useTranslation();

  // Peak hold for a stable dB read-out
  const peaks = usePeakHold(levelL, levelR, { holdTime: 500, releaseTime: 1500 });
  const maxPeak = isMuted ? 0 : Math.max(peaks.peakL, peaks.peakR);

  const percentL = isMuted ? 0 : clampPercent(levelL);
  const percentR = isMuted ? 0 : clampPercent(levelR);

  return (
    <div className={`master-section ${isMuted ? "master-section--muted" : ""}`}>
      <div className="master-section__header">
        <span className="master-section__title">{t("mixer.master")}</span>
        <span className="master-section__db">{levelToDb(maxPeak)} dB</span>
      </div>

      <div className="master-section__meters">
        {([
          ["L", percentL],
          ["R", percentR],
        ] as const).map(([label, percent]) => (
          <div className="master-section__row" key={label}>
            <span className="master-section__label">{label}</span>
            <div className="master-section__bar">
              <div className="master-section__fill" />
              <div
                className="master-section__mask"
                style={{ width: `${100 - percent}%` }}
              />
              <div className="master-section__zero" />
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

export default MasterSection;
