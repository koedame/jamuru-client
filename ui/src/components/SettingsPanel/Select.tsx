/**
 * Select - Dropdown select component
 *
 * Design: jamuru brand (ui.pen Screens/Settings). A native <select> is layered
 * at opacity 0 over a custom-styled value + chevron so we keep full native
 * keyboard/screen-reader behaviour while matching the flat dark visual.
 */

import { SelectHTMLAttributes } from "react";
import { ChevronDownIcon } from "./icons";
import "./Select.css";

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export type SelectVariant = "block" | "inline" | "channel";

export interface SelectProps
  extends Omit<SelectHTMLAttributes<HTMLSelectElement>, "onChange"> {
  /** Select options */
  options: SelectOption[];
  /** Placeholder text (shown when no option is selected) */
  placeholder?: string;
  /** Change handler */
  onChange?: (value: string) => void;
  /**
   * Visual variant.
   * - `block`: full-width field, primary-color value (device / language).
   * - `inline`: compact field, accent-color mono value (sample rate / buffer).
   * - `channel`: compact field with an internal prefix label (L/R channels).
   */
  variant?: SelectVariant;
  /** Prefix label shown inside the field (channel variant). */
  prefixLabel?: string;
}

export function Select({
  options,
  placeholder,
  value,
  onChange,
  disabled,
  variant = "block",
  prefixLabel,
  className,
  ...props
}: SelectProps) {
  const selectedOption = options.find((o) => o.value === value);
  const displayLabel = selectedOption?.label ?? placeholder ?? "";
  const isPlaceholder = !selectedOption;

  return (
    <div
      className={`select select--${variant} ${disabled ? "select--disabled" : ""} ${className ?? ""}`}
    >
      {prefixLabel && <span className="select__prefix">{prefixLabel}</span>}
      <span
        className={`select__value ${isPlaceholder ? "select__value--placeholder" : ""}`}
        aria-hidden="true"
      >
        {displayLabel}
      </span>
      <span className="select__chevron" aria-hidden="true">
        <ChevronDownIcon size={variant === "block" ? 14 : 12} />
      </span>
      <select
        className="select__native"
        value={value}
        onChange={(e) => onChange?.(e.target.value)}
        disabled={disabled}
        {...props}
      >
        {placeholder && (
          <option value="" disabled>
            {placeholder}
          </option>
        )}
        {options.map((option) => (
          <option
            key={option.value}
            value={option.value}
            disabled={option.disabled}
          >
            {option.label}
          </option>
        ))}
      </select>
    </div>
  );
}

export default Select;
