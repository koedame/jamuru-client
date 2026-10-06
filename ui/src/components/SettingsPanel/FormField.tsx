/**
 * FormField - form row layout component
 *
 * Design: jamuru brand (ui.pen Screens/Settings). Two layouts:
 * - `stacked`: label above the control, optional hint below (device / profile).
 * - `row`: title + description on the left, control on the right (audio settings).
 */

import { ReactNode } from "react";
import "./FormField.css";

export type FormFieldOrientation = "stacked" | "row";

export interface FormFieldProps {
  /** Field label */
  label: string;
  /** Field ID for accessibility */
  htmlFor?: string;
  /** Secondary description (shown under the label in `row` layout) */
  description?: string;
  /** Hint text shown below the control (`stacked` layout) */
  hint?: string;
  /** Error message */
  error?: string;
  /** Layout orientation */
  orientation?: FormFieldOrientation;
  /** Field content (input, select, etc.) */
  children: ReactNode;
}

export function FormField({
  label,
  htmlFor,
  description,
  hint,
  error,
  orientation = "stacked",
  children,
}: FormFieldProps) {
  return (
    <div
      className={`form-field form-field--${orientation} ${error ? "form-field--error" : ""}`}
    >
      <div className="form-field__label-group">
        <label className="form-field__label" htmlFor={htmlFor}>
          {label}
        </label>
        {description && <p className="form-field__description">{description}</p>}
      </div>
      <div className="form-field__content">
        {children}
        {hint && !error && <p className="form-field__hint">{hint}</p>}
        {error && <p className="form-field__error">{error}</p>}
      </div>
    </div>
  );
}

export default FormField;
