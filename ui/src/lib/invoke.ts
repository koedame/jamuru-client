/**
 * The UI's one way to call a Tauri command.
 *
 * Wraps the backend's `invoke` (`backend.ts`) so every failed command reaches
 * the log file with its name and error (ADR-036). Tauri does not let the
 * webview hook `invoke` itself: `__TAURI_INTERNALS__.invoke` is a read-only
 * property.
 */

import type { InvokeArgs, InvokeOptions } from "@tauri-apps/api/core";
import { invokeCommand } from "./backend";
import { recordCommandFailure, recordCommandOk } from "./logging";

export async function invoke<T>(
  cmd: string,
  args?: InvokeArgs,
  options?: InvokeOptions
): Promise<T> {
  const startedAt = performance.now();
  try {
    const result = await invokeCommand<T>(cmd, args, options);
    recordCommandOk(cmd, startedAt);
    return result;
  } catch (error) {
    recordCommandFailure(cmd, startedAt, error);
    throw error;
  }
}
