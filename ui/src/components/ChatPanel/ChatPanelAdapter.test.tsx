/**
 * Tests for the adapter's polling and sending: unchanged snapshots do not
 * rebuild the list, and a sent message shows without waiting for the next poll.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";

const invokeMock = vi.fn();
vi.mock("../../lib/invoke", () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));
vi.mock("../../lib/tauri", () => ({
  signalingToggleReaction: vi.fn(),
  signalingAddReaction: vi.fn(),
  chatOpenLink: vi.fn(),
}));

const seenMessages: unknown[] = [];
vi.mock("./ChatPanel", async () => {
  const actual = await vi.importActual<typeof import("./ChatPanel")>("./ChatPanel");
  return {
    ChatPanel: (props: Parameters<typeof actual.ChatPanel>[0]) => {
      seenMessages.push(props.messages);
      return actual.ChatPanel(props);
    },
  };
});

import { ChatPanelAdapter } from "./ChatPanelAdapter";

const sample = (id: string, content: string) => ({
  id,
  sender_id: "peer-b",
  sender_name: "Bob",
  content,
  timestamp: Date.now(),
  is_system: false,
  reactions: [],
});

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

describe("ChatPanelAdapter", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    seenMessages.length = 0;
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  // Verifies: REQ-GUI-030
  it("同じ内容が続けて届いたとき、一覧の状態が更新し直されないこと", async () => {
    invokeMock.mockResolvedValue([sample("m1", "hello")]);
    render(<ChatPanelAdapter connId={1} myPeerId="peer-a" />);
    await flush();
    expect(screen.getByText("hello")).toBeInTheDocument();
    const before = seenMessages.length;

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1600);
    });

    expect(invokeMock.mock.calls.length).toBeGreaterThan(2);
    expect(seenMessages.length).toBe(before);
  });

  // Verifies: REQ-GUI-030
  it("送信に成功したとき、次の定期取得を待たずに自分のメッセージが表示されること", async () => {
    let stored: ReturnType<typeof sample>[] = [];
    invokeMock.mockImplementation(async (cmd: string, args: { content?: string }) => {
      if (cmd === "signaling_send_chat") {
        stored = [{ ...sample("m1", args.content ?? ""), sender_id: "peer-a" }];
        return undefined;
      }
      return stored;
    });
    render(<ChatPanelAdapter connId={1} myPeerId="peer-a" />);
    await flush();

    const input = screen.getByTestId("chat-input");
    fireEvent.change(input, { target: { value: "hi" } });
    fireEvent.click(screen.getByTestId("chat-send"));
    await flush();
    await flush();

    expect(screen.getByText("hi")).toBeInTheDocument();
  });

  // Verifies: REQ-GUI-030
  it("送信が失敗したとき、エラーが表示され入力した文章が残ること", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "signaling_send_chat") throw new Error("offline");
      return [];
    });
    vi.spyOn(console, "error").mockImplementation(() => {});
    render(<ChatPanelAdapter connId={1} myPeerId="peer-a" />);
    await flush();

    const input = screen.getByTestId("chat-input") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "hi" } });
    fireEvent.click(screen.getByTestId("chat-send"));
    await flush();
    await flush();

    expect(screen.getByTestId("chat-send-error")).toBeInTheDocument();
    expect(input.value).toBe("hi");
  });
});
