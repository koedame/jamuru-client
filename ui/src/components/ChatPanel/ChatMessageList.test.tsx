/**
 * Tests for the message list: it follows new messages only while the reader is
 * at the bottom, and puts a date divider where the day changes.
 */
import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import { ChatMessageList, type ChatMessageData } from "./ChatMessageList";

const HOUR = 3_600_000;

function msg(id: string, timestamp: number, type: ChatMessageData["type"] = "other"): ChatMessageData {
  return { id, type, senderName: "Bob", content: id, timestamp };
}

function setGeometry(el: HTMLElement, scrollHeight: number, clientHeight: number) {
  Object.defineProperty(el, "scrollHeight", { configurable: true, value: scrollHeight });
  Object.defineProperty(el, "clientHeight", { configurable: true, value: clientHeight });
}

const render1 = (m: ChatMessageData) => <span>{m.content}</span>;

describe("ChatMessageList follow", () => {
  // Verifies: REQ-GUI-030
  it("読み返しているとき、更新で再描画されても位置が下へ戻されないこと", () => {
    const now = Date.now();
    const messages = [msg("a", now - 3 * HOUR), msg("b", now - 2 * HOUR)];
    const { container, rerender } = render(
      <ChatMessageList messages={messages} renderMessage={render1} />
    );
    const list = container.querySelector(".chat-message-list") as HTMLElement;
    setGeometry(list, 1000, 200);
    list.scrollTop = 100; // far from the bottom (800)
    fireEvent.scroll(list);

    // Same messages again (a poll that re-renders): the reader stays put.
    rerender(<ChatMessageList messages={[...messages]} renderMessage={render1} />);
    expect(list.scrollTop).toBe(100);

    // Even a new message from someone else does not pull them down.
    rerender(
      <ChatMessageList messages={[...messages, msg("c", now - HOUR)]} renderMessage={render1} />
    );
    expect(list.scrollTop).toBe(100);
  });

  // Verifies: REQ-GUI-030
  it("末尾にいるとき、新しいメッセージが来たら末尾まで追従すること", () => {
    const now = Date.now();
    const messages = [msg("a", now - 3 * HOUR)];
    const { container, rerender } = render(
      <ChatMessageList messages={messages} renderMessage={render1} />
    );
    const list = container.querySelector(".chat-message-list") as HTMLElement;
    setGeometry(list, 1000, 200);
    list.scrollTop = 790; // within the follow threshold of the bottom
    fireEvent.scroll(list);

    rerender(
      <ChatMessageList messages={[...messages, msg("b", now - HOUR)]} renderMessage={render1} />
    );
    expect(list.scrollTop).toBe(1000);
  });

  // Verifies: REQ-GUI-030
  it("読み返している途中で自分が送ったとき、末尾へ移ること", () => {
    const now = Date.now();
    const messages = [msg("a", now - 3 * HOUR)];
    const { container, rerender } = render(
      <ChatMessageList messages={messages} renderMessage={render1} />
    );
    const list = container.querySelector(".chat-message-list") as HTMLElement;
    setGeometry(list, 1000, 200);
    list.scrollTop = 100;
    fireEvent.scroll(list);

    rerender(
      <ChatMessageList
        messages={[...messages, msg("mine", now, "own")]}
        renderMessage={render1}
      />
    );
    expect(list.scrollTop).toBe(1000);
  });
});

describe("ChatMessageList date dividers", () => {
  // Verifies: REQ-GUI-030
  it("日をまたぐメッセージの間に、日付の区切りが入ること", () => {
    const day1 = new Date(2020, 0, 5, 23, 50).getTime();
    const day2 = new Date(2020, 0, 6, 0, 10).getTime();
    render(
      <ChatMessageList
        messages={[msg("a", day1), msg("b", day1 + 1000), msg("c", day2)]}
        renderMessage={render1}
        locale="en-US"
      />
    );

    const dividers = screen.getAllByTestId("chat-date-separator");
    // One before the first (not today), one where the day changes.
    expect(dividers).toHaveLength(2);
    expect(dividers[0]).toHaveTextContent("January 5, 2020");
    expect(dividers[1]).toHaveTextContent("January 6, 2020");
  });

  // Verifies: REQ-GUI-030
  it("すべてのメッセージが今日のとき、日付の区切りが出ないこと", () => {
    const now = new Date();
    const noon = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 0, 0, 1).getTime();
    render(
      <ChatMessageList
        messages={[msg("a", noon), msg("b", noon + 1000)]}
        renderMessage={render1}
      />
    );
    expect(screen.queryByTestId("chat-date-separator")).toBeNull();
  });
});
