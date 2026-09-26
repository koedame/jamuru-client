/**
 * PanSlider - Horizontal pan control
 *
 * Range: -100 (full left) to 100 (full right), 0 = center.
 * jamjam brand (ui.pen Molecules/Slider/Pan): the pan value label ("C"/"L10"/
 * "R25") sits at the top-center of the control, with a yellow thumb and a green
 * fill from the center detent to the thumb.
 */

import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import "./PanSlider.css";

export interface PanSliderProps {
  /** Pan value (-100 to 100) */
  value: number;
  /** Whether the slider is disabled */
  disabled?: boolean;
  /** Callback when pan changes */
  onChange?: (value: number) => void;
  /** Accessibility label */
  label?: string;
}

/** Format pan value for display (C = center, L/R with magnitude). */
function formatPan(pan: number): string {
  if (Math.abs(pan) <= 2) return "C";
  if (pan < 0) return `L${Math.abs(pan)}`;
  return `R${pan}`;
}

export function PanSlider({
  value,
  disabled = false,
  onChange,
  label: labelProp,
}: PanSliderProps) {
  const { t } = useTranslation();
  const label = labelProp ?? t("mixer.channel.pan");
  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const newValue = parseInt(e.target.value, 10);
      onChange?.(newValue);
    },
    [onChange]
  );

  // Double-click to reset to center
  const handleDoubleClick = useCallback(() => {
    if (!disabled) {
      onChange?.(0);
    }
  }, [disabled, onChange]);

  // Thumb position: 0% (full left) .. 50% (center) .. 100% (full right)
  const thumbPosition = ((value + 100) / 200) * 100;
  // Green indicator spans from the center detent to the thumb
  const indicatorLeft = Math.min(50, thumbPosition);
  const indicatorWidth = Math.abs(thumbPosition - 50);

  return (
    <div
      className={`pan-slider ${disabled ? "pan-slider--disabled" : ""}`}
      onDoubleClick={handleDoubleClick}
    >
      <span className="pan-slider__value">{formatPan(value)}</span>
      <input
        type="range"
        min="-100"
        max="100"
        value={value}
        onChange={handleChange}
        disabled={disabled}
        className="pan-slider__input"
        data-testid="channel-pan"
        aria-label={label}
        aria-valuemin={-100}
        aria-valuemax={100}
        aria-valuenow={value}
      />
      <div className="pan-slider__track">
        <div className="pan-slider__center" />
        <div
          className="pan-slider__indicator"
          style={{ left: `${indicatorLeft}%`, width: `${indicatorWidth}%` }}
        />
        <div
          className="pan-slider__thumb"
          style={{ left: `${thumbPosition}%` }}
        />
      </div>
    </div>
  );
}

export default PanSlider;
