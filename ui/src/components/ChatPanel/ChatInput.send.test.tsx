/**
 * Tests for how the input treats the result of a send: the text is kept until
 * the send succeeded, and a failure is shown instead of being lost.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import { ChatInput } from "./ChatInput";

function typeAndSend(text: string) {
  const input = screen.getByTestId("chat-input") as HTMLTextAreaElement;
  fireEvent.change(input, { target: { value: text } });
  fireEvent.click(screen.getByTestId("chat-send"));
  return input;
}

describe("ChatInput send result", () => {
  // Verifies: REQ-GUI-030
  it("送信に失敗したとき、入力した文章が残りエラーが表示されること", async () => {
    const onSend = vi.fn().mockRejectedValue(new Error("offline"));
    render(<ChatInput onSend={onSend} />);

    const input = typeAndSend("こんにちは");

    expect(await screen.findByTestId("chat-send-error")).toBeInTheDocument();
    expect(input.value).toBe("こんにちは");
  });

  // Verifies: REQ-GUI-030
  it("送信に成功したとき、入力欄が空になりエラーが出ないこと", async () => {
    const onSend = vi.fn().mockResolvedValue(undefined);
    render(<ChatInput onSend={onSend} />);

    const input = typeAndSend("こんにちは");

    await waitFor(() => expect(input.value).toBe(""));
    expect(onSend).toHaveBeenCalledWith("こんにちは");
    expect(screen.queryByTestId("chat-send-error")).toBeNull();
  });

  // Verifies: REQ-GUI-030
  it("失敗のあとに文章を直し始めたとき、エラーの表示が消えること", async () => {
    const onSend = vi.fn().mockRejectedValue(new Error("offline"));
    render(<ChatInput onSend={onSend} />);

    const input = typeAndSend("a");
    await screen.findByTestId("chat-send-error");
    fireEvent.change(input, { target: { value: "ab" } });

    expect(screen.queryByTestId("chat-send-error")).toBeNull();
  });

  // Verifies: REQ-GUI-030
  it("送信の結果を待っている間、同じ文章が二重に送られないこと", async () => {
    let resolve!: () => void;
    const onSend = vi.fn(() => new Promise<void>((r) => (resolve = r)));
    render(<ChatInput onSend={onSend} />);

    typeAndSend("x");
    fireEvent.click(screen.getByTestId("chat-send"));
    expect(onSend).toHaveBeenCalledTimes(1);

    resolve();
    await waitFor(() => expect((screen.getByTestId("chat-input") as HTMLTextAreaElement).value).toBe(""));
  });
});
