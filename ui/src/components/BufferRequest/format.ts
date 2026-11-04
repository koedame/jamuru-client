/**
 * How the receive buffer is worded on screen (ADR-097). The core counts in
 * tenths of a millisecond; the screen says milliseconds with one decimal.
 */

/** Tenths of a millisecond as milliseconds, no trailing zero ("5", "2.5") */
export function msText(ms10: number): string {
  return String(Math.round(ms10) / 10);
}

/**
 * What the user typed (milliseconds, decimal point or comma) as tenths of a
 * millisecond, or null when it is not a number within 0.1 to `maxMs10`.
 */
export function parseMs10(text: string, maxMs10: number): number | null {
  const value = Number(text.trim().replace(",", "."));
  if (text.trim() === "" || !Number.isFinite(value)) return null;
  const ms10 = Math.round(value * 10);
  return ms10 >= 1 && ms10 <= maxMs10 ? ms10 : null;
}

/** What the user typed as a count of tenths, within 0 to `max`; null when it is not */
export function parseTenths(text: string, max: number): number | null {
  const value = Number(text.trim().replace(",", "."));
  if (text.trim() === "" || !Number.isFinite(value)) return null;
  const tenths = Math.round(value * 10);
  return tenths >= 0 && tenths <= max ? tenths : null;
}
