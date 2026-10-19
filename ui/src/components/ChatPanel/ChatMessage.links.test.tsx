/**
 * Tests for the links in a chat message: they are clickable, and the click is
 * handed to the app instead of navigating the app's own window.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import { ChatMessage } from "./ChatMessage";

describe("ChatMessage links", () => {
  // Verifies: REQ-GUI-029
  it("本文にアドレスがあるとき、リンクとして表示され本文の文字は変わらないこと", () => {
    const { container } = render(
      <ChatMessage type="other" senderName="Bob" content="見て https://example.com/a です" timestamp={0} />
    );

    const link = screen.getByRole("link", { name: "https://example.com/a" });
    expect(link).toHaveAttribute("href", "https://example.com/a");
    expect(container.querySelector(".chat-message__content")).toHaveTextContent(
      "見て https://example.com/a です"
    );
  });

  // Verifies: REQ-GUI-029
  it("リンクをクリックしたとき、ウィンドウは遷移せずアドレスが onOpenLink に渡ること", () => {
    const onOpenLink = vi.fn();
    render(
      <ChatMessage type="other" senderName="Bob" content="https://example.com/a" timestamp={0} onOpenLink={onOpenLink} />
    );

    const notPrevented = fireEvent.click(screen.getByRole("link"));

    expect(notPrevented).toBe(false);
    expect(onOpenLink).toHaveBeenCalledWith("https://example.com/a");
  });

  // Verifies: REQ-GUI-029
  it("本文が http と https 以外のとき、リンクにならないこと", () => {
    render(<ChatMessage type="other" senderName="Bob" content="file:///etc/passwd javascript:alert(1)" timestamp={0} />);

    expect(screen.queryByRole("link")).toBeNull();
  });

  // Verifies: REQ-GUI-029
  it("自分のメッセージにアドレスがあるとき、同じようにリンクになること", () => {
    render(<ChatMessage type="own" content="https://example.com" timestamp={0} />);

    expect(screen.getByRole("link")).toHaveAttribute("href", "https://example.com");
  });
});
