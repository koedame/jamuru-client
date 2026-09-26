/**
 * StereoFader - Vertical volume fader
 *
 * jamjam brand (ui.pen Molecules/Fader/Stereo): recessed vertical track with a
 * green glow fill from the bottom up to the thumb, a light rectangular thumb,
 * and a 0 dB reference line (unity gain = volume 80).
 */

import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { clampPercent } from "./mixerScale";
import "./StereoFader.css";

export interface StereoFaderProps {
  /** Volume value (0-100) */
  volume: number;
  /** Height in pixels */
  height?: number;
  /** Whether the fader is disabled */
  disabled?: boolean;
  /** Callback when volume changes */
  onChange?: (volume: number) => void;
  /** Accessibility label */
  label?: string;
}

/** 0 dB (unity gain) reference position on the 0-100 fader scale. */
const ZERO_DB_PERCENT = 80;

export function StereoFader({
  volume,
  height = 200,
  disabled = false,
  onChange,
  label: labelProp,
}: StereoFaderProps) {
  const { t } = useTranslation();
  const label = labelProp ?? t("mixer.channel.volume");
  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const newVolume = parseInt(e.target.value, 10);
      onChange?.(newVolume);
    },
    [onChange]
  );

  // Double-click to reset to 0dB (volume = 80)
  const handleDoubleClick = useCallback(() => {
    if (!disabled) {
      onChange?.(ZERO_DB_PERCENT);
    }
  }, [disabled, onChange]);

  // Keyboard handling
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (disabled) return;

      let newVolume = volume;
      switch (e.key) {
        case "PageUp":
          newVolume = Math.min(100, volume + 10);
          e.preventDefault();
          break;
        case "PageDown":
          newVolume = Math.max(0, volume - 10);
          e.preventDefault();
          break;
        case "Home":
          newVolume = 100;
          e.preventDefault();
          break;
        case "End":
          newVolume = 0;
          e.preventDefault();
          break;
      }
      if (newVolume !== volume) {
        onChange?.(newVolume);
      }
    },
    [disabled, volume, onChange]
  );

  const fillPercent = clampPercent(volume);

  return (
    <div
      className={`stereo-fader ${disabled ? "stereo-fader--disabled" : ""}`}
      style={{ height: `${height}px` }}
      onDoubleClick={handleDoubleClick}
    >
      {/* Hidden range input for accessibility and interaction */}
      <input
        type="range"
        min="0"
        max="100"
        value={volume}
        onChange={handleChange}
        onKeyDown={handleKeyDown}
        disabled={disabled}
        className="stereo-fader__input"
        data-testid="channel-fader"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={volume}
      />

      {/* Recessed track with green glow fill and 0 dB reference line */}
      <div className="stereo-fader__track">
        <div
          className="stereo-fader__glow"
          style={{ height: `${fillPercent}%` }}
        />
        <div
          className="stereo-fader__zero"
          style={{ bottom: `${ZERO_DB_PERCENT}%` }}
        />
      </div>

      <div className="stereo-fader__thumb" style={{ bottom: `${fillPercent}%` }} />
    </div>
  );
}

export default StereoFader;
