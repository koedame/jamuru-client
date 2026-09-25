/**
 * The backend a helper's window is connected to (ADR-044 §5): the app being
 * helped, through the relay.
 *
 * The window draws the same screens as the app's own, but every command it
 * calls goes to the other app (`help_call`, which sends it over the relay) and
 * every event it hears is one the other app announced. The other app decides
 * what may be called; the refusals come back as errors, so nothing here needs to
 * know the range. What stays this window's own is what is about the window
 * itself or the helper: its language, its log, its size.
 */

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import type { Backend } from "./backend";

/** The URL hash of the window someone helping works in (`help_link.rs` opens it). */
export const HELP_HASH = "#/help";

/** What the helper's window hears the other app's events as; the payload is `{ name, payload }`. */
export const REMOTE_EVENT = "help-remote:event";

/** Commands that are this window's own, not the helped app's. */
const OWN_COMMANDS = new Set([
  "help_call",
  "help_window_info",
  "config_get_language",
  "config_set_language",
  "log_frontend",
]);

/** Commands about the window itself (`window_*`) are its own too. */
const OWN_COMMAND_PREFIXES = ["window_"];

/** Events that are this window's own: the helper's language is the helper's. */
const OWN_EVENTS = new Set(["i18n:language-changed"]);

export function isOwnCommand(cmd: string): boolean {
  return OWN_COMMANDS.has(cmd) || OWN_COMMAND_PREFIXES.some((prefix) => cmd.startsWith(prefix));
}

export const helperBackend: Backend = {
  invoke: (cmd, args, options) =>
    isOwnCommand(cmd)
      ? tauriInvoke(cmd, args, options)
      : tauriInvoke("help_call", { method: cmd, params: args ?? {} }, options),
  listen: (event, handler) =>
    OWN_EVENTS.has(event)
      ? tauriListen(event, (e) => handler(e.payload as never))
      : tauriListen<{ name: string; payload: unknown }>(REMOTE_EVENT, (e) => {
          if (e.payload.name === event) handler(e.payload.payload as never);
        }),
};
