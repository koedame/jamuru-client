import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const listen = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: (...args: unknown[]) => listen(...args) }));

import { helperBackend, isOwnCommand, REMOTE_EVENT } from "./helperBackend";

beforeEach(() => {
  invoke.mockReset();
  listen.mockReset();
});

describe("the backend a helper's window is connected to", () => {
  it("a command about the audio is sent to the helped app with its arguments", async () => {
    invoke.mockResolvedValue({ ok: true });

    const result = await helperBackend.invoke("streaming_set_mute", { muted: true });

    expect(result).toEqual({ ok: true });
    expect(invoke).toHaveBeenCalledWith(
      "help_call",
      { method: "streaming_set_mute", params: { muted: true } },
      undefined
    );
  });

  it("a command without arguments is sent with none", async () => {
    await helperBackend.invoke("session_get");

    expect(invoke).toHaveBeenCalledWith("help_call", { method: "session_get", params: {} }, undefined);
  });

  it("a command the helped app refuses fails the call with what it said", async () => {
    invoke.mockRejectedValue({ code: "denied", message: "Help may not call signaling_send_chat" });

    await expect(helperBackend.invoke("signaling_send_chat", { connId: 1 })).rejects.toEqual({
      code: "denied",
      message: "Help may not call signaling_send_chat",
    });
  });

  it("the window's own language and size are not the helped app's to answer", async () => {
    for (const cmd of ["config_get_language", "config_set_language", "window_resize_main", "log_frontend"]) {
      invoke.mockClear();
      await helperBackend.invoke(cmd, { language: "ja" });
      expect(invoke).toHaveBeenCalledWith(cmd, { language: "ja" }, undefined);
    }
    expect(isOwnCommand("streaming_status")).toBe(false);
    expect(isOwnCommand("settings_change")).toBe(false);
  });

  it("an event of the helped app reaches the handler with its payload, and one with another name does not", async () => {
    let deliver: (event: { payload: unknown }) => void = () => {};
    listen.mockImplementation((_event: string, callback: typeof deliver) => {
      deliver = callback;
      return Promise.resolve(() => {});
    });
    const heard: unknown[] = [];

    await helperBackend.listen("session:changed", (payload) => heard.push(payload));
    deliver({ payload: { name: "audio:config-changed", payload: { revision: 1 } } });
    deliver({ payload: { name: "session:changed", payload: { revision: 7 } } });

    expect(listen.mock.calls[0][0]).toBe(REMOTE_EVENT);
    expect(heard).toEqual([{ revision: 7 }]);
  });

  it("the language event is the window's own, heard as the app emits it", async () => {
    let deliver: (event: { payload: unknown }) => void = () => {};
    listen.mockImplementation((_event: string, callback: typeof deliver) => {
      deliver = callback;
      return Promise.resolve(() => {});
    });
    const heard: unknown[] = [];

    await helperBackend.listen("i18n:language-changed", (payload) => heard.push(payload));
    deliver({ payload: "ja" });

    expect(listen.mock.calls[0][0]).toBe("i18n:language-changed");
    expect(heard).toEqual(["ja"]);
  });
});
