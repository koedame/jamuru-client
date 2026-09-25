/**
 * The screen's one way to reach the backend: call a command, hear an event.
 *
 * The screen never imports Tauri's `invoke` / `listen` itself; it goes through
 * here, so what it is connected to can be swapped. The app's own windows are
 * connected to this app's backend. A window that helps another participant is
 * connected to *their* backend, through the relay (`helperBackend.ts`), and draws
 * the same screens from it (ADR-044 §5). Each window is its own webview with its
 * own copy of this module, so switching one leaves the others as they are.
 */

import {
  invoke as tauriInvoke,
  type InvokeArgs,
  type InvokeOptions,
} from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";

export interface Backend {
  invoke<T>(cmd: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T>;
  /** Hear `event` until the returned function is called */
  listen<T>(event: string, handler: (payload: T) => void): Promise<() => void>;
}

/** This app's own backend, over Tauri's IPC. */
export const tauriBackend: Backend = {
  invoke: (cmd, args, options) => tauriInvoke(cmd, args, options),
  listen: (event, handler) => tauriListen(event, (e) => handler(e.payload as never)),
};

let current: Backend = tauriBackend;

/** Connects this window's screen to `backend`. Call before the screen mounts. */
export function setBackend(backend: Backend): void {
  current = backend;
}

export function invokeCommand<T>(
  cmd: string,
  args?: InvokeArgs,
  options?: InvokeOptions
): Promise<T> {
  return current.invoke<T>(cmd, args, options);
}

/** Hear `event` from the backend this window is connected to. */
export function listenEvent<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
  return current.listen<T>(event, handler);
}
