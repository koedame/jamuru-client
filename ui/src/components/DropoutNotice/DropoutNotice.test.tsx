/**
 * DropoutNotice component tests (REQ-CON-162, REQ-CON-163)
 */

import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { DropoutNotice } from "./DropoutNotice";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { seconds?: number }) => {
      const translations: Record<string, string> = {
        "dropout.heading": "No audio arriving",
        "dropout.seconds": `${options?.seconds} s`,
        "dropout.side.mine": "Your line",
        "dropout.side.theirs": "Their line",
        "dropout.side.between": "Between you",
        "dropout.side.checking": "Checking",
        "dropout.unstable": "Unstable",
        "dropout.unstableHint": "Cut out twice or more",
      };
      return translations[key] ?? key;
    },
  }),
}));

describe("DropoutNotice", () => {
  /** Verifies: REQ-CON-161 */
  it("shows nothing when audio arrives and the link is steady", () => {
    const { container } = render(
      <DropoutNotice dropout={null} unstable={false} variant="row" />
    );
    expect(container).toBeEmptyDOMElement();
  });

  /** Verifies: REQ-CON-161 */
  it("says no audio arrives and for how many seconds when the audio stopped", () => {
    render(
      <DropoutNotice dropout={{ silent_secs: 12, side: "theirs" }} unstable={false} variant="row" />
    );
    expect(screen.getByTestId("dropout-heading")).toHaveTextContent("No audio arriving");
    expect(screen.getByTestId("dropout-seconds")).toHaveTextContent("12 s");
  });

  /** Verifies: REQ-CON-161, REQ-CON-163 */
  it.each([
    ["mine", "Your line"],
    ["theirs", "Their line"],
    ["between", "Between you"],
    ["checking", "Checking"],
  ] as const)("names whose line is cut when the side is %s", (side, text) => {
    render(<DropoutNotice dropout={{ silent_secs: 5, side }} unstable={false} variant="row" />);
    expect(screen.getByTestId("dropout-side")).toHaveTextContent(text);
  });

  /** Verifies: REQ-CON-161, REQ-CON-163 */
  it("names no side when nobody could tell which is cut", () => {
    render(
      <DropoutNotice dropout={{ silent_secs: 45, side: "unknown" }} unstable={false} variant="row" />
    );
    expect(screen.getByTestId("dropout-heading")).toBeInTheDocument();
    expect(screen.queryByTestId("dropout-side")).not.toBeInTheDocument();
  });

  /** Verifies: REQ-CON-161 */
  it("marks the link unstable when it dropped out twice or more, with no dropout going on", () => {
    render(<DropoutNotice dropout={null} unstable variant="row" />);
    expect(screen.getByTestId("dropout-unstable")).toHaveTextContent("Unstable");
    expect(screen.queryByTestId("dropout-heading")).not.toBeInTheDocument();
  });

  /** Verifies: REQ-CON-161 */
  it("keeps the seconds out of what a screen reader announces when they count up", () => {
    render(
      <DropoutNotice dropout={{ silent_secs: 3, side: "mine" }} unstable={false} variant="strip" />
    );
    expect(screen.getByTestId("dropout-seconds")).toHaveAttribute("aria-hidden", "true");
  });
});
