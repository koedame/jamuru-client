import { describe, expect, it } from "vitest";
import { formatRemaining } from "./useHelpRemaining";

describe("formatRemaining", () => {
  // Verifies: REQ-RMT-037
  it("a whole number of minutes is left, it reads minutes and two zero seconds", () => {
    expect(formatRemaining(30 * 60 * 1000)).toBe("30:00");
  });

  // Verifies: REQ-RMT-037
  it("part of a second is left, it counts as the whole second", () => {
    expect(formatRemaining(59_001)).toBe("1:00");
  });

  // Verifies: REQ-RMT-037
  it("seconds are below ten, they are padded to two digits", () => {
    expect(formatRemaining(65_000)).toBe("1:05");
  });

  // Verifies: REQ-RMT-037
  it("the time has run out or is negative, it reads zero", () => {
    expect(formatRemaining(0)).toBe("0:00");
    expect(formatRemaining(-4_000)).toBe("0:00");
  });
});
