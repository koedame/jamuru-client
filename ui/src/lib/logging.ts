/**
 * Webview side of the diagnostic log file (ADR-036).
 *
 * An installed app has no developer tools, so the webview's `console` output,
 * uncaught errors and failed Tauri commands are sent to the app's log file
 * through the `log_frontend` command. That gives one file that shows the UI's
 * state changes next to what the Rust side did.
 */

import { invoke, isTauri } from "@tauri-apps/api/core";

type Level = "error" | "warn" | "info" | "debug";

/** The app command that appends a line to the log file. */
const LOG_COMMAND = "log_frontend";

/**
 * Commands the UI calls on a timer (100-500ms). Their successes are not worth
 * a line each; their failures repeat every tick and are limited like any other
 * repeated line.
 */
const POLLED_COMMANDS = new Set([
  "streaming_status",
  "streaming_get_input_level",
  "signaling_get_chat_messages",
]);

/**
 * A line repeats within this window at most this many times; the rest are
 * counted and reported on the next line of the same kind. A failing poll
 * would otherwise write a line every 100ms and push everything useful out of
 * the rotated file.
 */
const REPEAT_WINDOW_MS = 10_000;
const REPEATS_PER_WINDOW = 3;

/** Text for one `console` argument. Never throws: a log call must not break the caller. */
function formatValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (value instanceof Error) return value.stack ?? `${value.name}: ${value.message}`;
  if (typeof value === "object" && value !== null) {
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  }
  return String(value);
}

export function formatConsoleArgs(args: unknown[]): string {
  return args.map(formatValue).join(" ");
}

/** Tauri rejects a failed command with the string the Rust side returned. */
function describeError(error: unknown): string {
  return error instanceof Error ? error.message : formatValue(error);
}

/** Above this many remembered lines, the ones whose window has passed are forgotten. */
const MAX_REMEMBERED_LINES = 200;

/**
 * Decides which lines are written, and how many were held back before them.
 * Only identical lines (same `key`) count against each other.
 */
export function createRepeatLimiter(now: () => number) {
  const seen = new Map<string, { windowStart: number; written: number; held: number }>();

  return (key: string): { write: boolean; heldBack: number } => {
    if (seen.size > MAX_REMEMBERED_LINES) {
      for (const [remembered, entry] of seen) {
        if (now() - entry.windowStart >= REPEAT_WINDOW_MS) seen.delete(remembered);
      }
    }
    const entry = seen.get(key);
    if (!entry || now() - entry.windowStart >= REPEAT_WINDOW_MS) {
      seen.set(key, { windowStart: now(), written: 1, held: 0 });
      return { write: true, heldBack: entry?.held ?? 0 };
    }
    if (entry.written < REPEATS_PER_WINDOW) {
      entry.written += 1;
      return { write: true, heldBack: 0 };
    }
    entry.held += 1;
    return { write: false, heldBack: 0 };
  };
}

let shouldWrite: ReturnType<typeof createRepeatLimiter> | null = null;

/**
 * Lines are sent one at a time: `log_frontend` is an async command, and
 * commands sent together may run in any order, which would scramble the
 * sequence of states the file exists to show.
 */
let sending: Promise<unknown> = Promise.resolve();

/** Writes one line. Does nothing until `installFrontendLogging` ran. */
function record(level: Level, message: string, key: string = `${level} ${message}`): void {
  if (!shouldWrite) return;
  const { write, heldBack } = shouldWrite(key);
  if (!write) return;
  const line = heldBack > 0 ? `${message} (${heldBack} similar line(s) held back)` : message;
  // A failure here is dropped: there is nowhere left to report it, and
  // reporting it through `console` would loop back into this function.
  sending = sending
    .then(() => invoke(LOG_COMMAND, { level, message: line }))
    .catch(() => {});
}

function elapsedMs(startedAt: number): number {
  return Math.round(performance.now() - startedAt);
}

export function recordCommandOk(cmd: string, startedAt: number): void {
  if (POLLED_COMMANDS.has(cmd)) return;
  record("debug", `invoke ${cmd} ok (${elapsedMs(startedAt)}ms)`, `invoke ${cmd} ok`);
}

/**
 * The arguments are not logged: they carry room passwords and chat text.
 * Tauri refusing a call for lack of a capability arrives here too, as an error
 * that names the command.
 */
export function recordCommandFailure(cmd: string, startedAt: number, error: unknown): void {
  const reason = describeError(error);
  record(
    "error",
    `invoke ${cmd} failed (${elapsedMs(startedAt)}ms): ${reason}`,
    `invoke ${cmd} failed: ${reason}`
  );
}

/**
 * Starts sending the webview's console output and uncaught errors to the log
 * file. Returns a function that undoes it. Does nothing outside a Tauri
 * webview (Storybook, browser preview).
 */
export function installFrontendLogging(): () => void {
  if (!isTauri()) return () => {};

  shouldWrite = createRepeatLimiter(() => Date.now());

  const consoleLevels = [
    ["log", "info"],
    ["info", "info"],
    ["warn", "warn"],
    ["error", "error"],
    ["debug", "debug"],
  ] as const;
  const originalConsole = consoleLevels.map(([method]) => console[method]);
  consoleLevels.forEach(([method, level], i) => {
    console[method] = (...args: unknown[]) => {
      originalConsole[i].apply(console, args);
      record(level, formatConsoleArgs(args));
    };
  });

  const onError = (event: ErrorEvent) => {
    const where = event.filename ? ` at ${event.filename}:${event.lineno}:${event.colno}` : "";
    const stack = event.error instanceof Error && event.error.stack ? `\n${event.error.stack}` : "";
    record("error", `Uncaught error: ${event.message}${where}${stack}`);
  };
  const onRejection = (event: PromiseRejectionEvent) => {
    record("error", `Unhandled promise rejection: ${formatValue(event.reason)}`);
  };
  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onRejection);

  return () => {
    shouldWrite = null;
    consoleLevels.forEach(([method], i) => {
      console[method] = originalConsole[i];
    });
    window.removeEventListener("error", onError);
    window.removeEventListener("unhandledrejection", onRejection);
  };
}
