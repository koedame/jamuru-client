/**
 * SettingsHelpQuestion (ADR-043): a question that has just appeared does not
 * take Allow, from the very frame it is drawn.
 */

import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { useLayoutEffect } from "react";

import { SettingsHelpQuestion } from "./SettingsHelpQuestion";

/**
 * Reads Allow's state right after the question is drawn and before its own
 * effects run - the frame a click could land in.
 */
function FirstFrame({ open, onSeen }: { open: boolean; onSeen: (held: string | null) => void }) {
  useLayoutEffect(() => {
    onSeen(document.querySelector("[data-testid='settings-help-allow']")?.getAttribute("aria-disabled") ?? null);
  });
  return (
    <SettingsHelpQuestion
      open={open}
      message="Aki wants to help with your audio settings"
      allowLabel="Allow"
      declineLabel="Decline"
      onAllow={() => {}}
      onDecline={() => {}}
    />
  );
}

describe("SettingsHelpQuestion", () => {
  // Verifies: REQ-RMT-002
  it("a question drawn for the first time holds Allow back from its first frame", () => {
    const seen: (string | null)[] = [];
    render(<FirstFrame open onSeen={(held) => seen.push(held)} />);

    expect(seen[0]).toBe("true");
  });

  // Verifies: REQ-RMT-002
  it("a question shown again after it closed holds Allow back from its first frame again", async () => {
    const seen: (string | null)[] = [];
    const onSeen = (held: string | null) => seen.push(held);
    const { rerender } = render(<FirstFrame open onSeen={onSeen} />);
    await new Promise((resolve) => setTimeout(resolve, 600));
    rerender(<FirstFrame open onSeen={onSeen} />);
    expect(seen[seen.length - 1]).toBe("false");

    rerender(<FirstFrame open={false} onSeen={onSeen} />);
    rerender(<FirstFrame open onSeen={onSeen} />);

    expect(seen[seen.length - 1]).toBe("true");
  });
});
