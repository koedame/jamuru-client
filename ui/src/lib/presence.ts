/**
 * Where a person of a settings help is looking, as a screen can tell it
 * (ADR-044 §5, REQ-RMT-034).
 *
 * Both people see the same screen, so each one's screen marks the part the other
 * is pointing at and the part the other has focused. Only parts that can be
 * pressed or focused count, which is what a keyboard user and a screen reader
 * also move between, so the same notion is what accessibility needs later. What
 * is sent is a part's name and a place in it; the pointer's path is not, and
 * neither is anything outside the window.
 */

import { cssPath } from "./domTarget";
import type { Presence, PresenceTarget } from "./tauri";

/** The most messages a screen sends in a second */
export const MAX_SENDS_PER_SECOND = 10;

const SEND_INTERVAL_MS = 1000 / MAX_SENDS_PER_SECOND;

/** The longest name the other side accepts */
const MAX_ID_CHARS = 200;

/** What a person presses or moves the focus to. `data-presence-id` opts any other element in. */
const INTERACTIVE = [
  "a[href]",
  "button",
  'input:not([type="hidden"])',
  "select",
  "textarea",
  "summary",
  '[contenteditable="true"]',
  '[tabindex]:not([tabindex="-1"])',
  "[data-presence-id]",
  ...["button", "checkbox", "radio", "switch", "tab", "menuitem", "option", "slider", "link", "combobox", "textbox", "spinbutton"].map(
    (role) => `[role="${role}"]`
  ),
].join(",");

function isDisabled(el: Element): boolean {
  return el.matches(":disabled") || el.getAttribute("aria-disabled") === "true";
}

/** The part that can be pressed or focused that `el` is, or is inside; null when there is none. */
export function interactiveOf(el: Element | null): Element | null {
  const part = el?.closest(INTERACTIVE) ?? null;
  return part && !isDisabled(part) ? part : null;
}

/**
 * The part's name. Its own `data-presence-id` first, then its `data-testid`; a part with neither
 * gets a path of tags. Never its text: the two people can read different languages.
 */
export function presenceIdOf(el: Element): string | null {
  const id = el.getAttribute("data-presence-id") ?? el.getAttribute("data-testid") ?? cssPath(el, { stable: true });
  return id && id.length <= MAX_ID_CHARS ? id : null;
}

/** The part named `id` on this screen, if it is there. */
export function findPresenceElement(id: string): Element | null {
  for (const part of document.querySelectorAll(INTERACTIVE)) {
    if (presenceIdOf(part) === id) return part;
  }
  return null;
}

const clamp01 = (value: number) => Math.min(1, Math.max(0, value));
const hundredth = (value: number) => Math.round(value * 100) / 100;

/** `part` as a target, with the place `(clientX, clientY)` inside it. */
export function targetOf(part: Element, clientX: number, clientY: number): PresenceTarget | null {
  const id = presenceIdOf(part);
  if (id === null) return null;
  const rect = part.getBoundingClientRect();
  return {
    id,
    x: rect.width > 0 ? hundredth(clamp01((clientX - rect.left) / rect.width)) : 0.5,
    y: rect.height > 0 ? hundredth(clamp01((clientY - rect.top) / rect.height)) : 0.5,
  };
}

function sameTarget(a: PresenceTarget | null, b: PresenceTarget | null): boolean {
  return a === b || (a !== null && b !== null && a.id === b.id && a.x === b.x && a.y === b.y);
}

/** The part with the focus, as a target at its middle; null when the focus is on none. */
function focusTarget(): PresenceTarget | null {
  const part = interactiveOf(document.activeElement);
  if (part === null) return null;
  const rect = part.getBoundingClientRect();
  return targetOf(part, rect.left + rect.width / 2, rect.top + rect.height / 2);
}

export interface PresenceWatch {
  stop: () => void;
  /** Sends the picture as it is now (at the allowed rate), even if nothing moved */
  refresh: () => void;
}

/**
 * Watches where the pointer and the focus are and calls `send` with the
 * picture when it changes, at most [`MAX_SENDS_PER_SECOND`] times a second.
 * The last change is never dropped, only held until the next send is allowed.
 * The pointer counts only while it is over a part that can be pressed or
 * focused: moving over the rest of the window says nothing.
 */
export function watchPresence(send: (presence: Presence) => void, extra: () => Partial<Presence> = () => ({})): PresenceWatch {
  let hover: PresenceTarget | null = null;
  let focus: PresenceTarget | null = focusTarget();
  let last: Presence | null = null;
  let lastSentAt = Number.NEGATIVE_INFINITY;
  let timer: number | undefined;

  const picture = (): Presence => ({ hover, focus, ...extra() });
  const same = (a: Presence, b: Presence) =>
    sameTarget(a.hover, b.hover) && sameTarget(a.focus, b.focus) && a.panel === b.panel;

  function flush(force: boolean): void {
    timer = undefined;
    const now = picture();
    if (!force && last !== null && same(last, now)) return;
    const wait = lastSentAt + SEND_INTERVAL_MS - performance.now();
    if (wait > 0) {
      timer = window.setTimeout(() => flush(force), wait);
      return;
    }
    lastSentAt = performance.now();
    last = now;
    send(now);
  }

  function changed(force = false): void {
    if (timer !== undefined) {
      // A send is already waiting for its turn and will take what is current then.
      return;
    }
    flush(force);
  }

  function onMove(event: MouseEvent): void {
    const part = event.target instanceof Element ? interactiveOf(event.target) : null;
    hover = part === null ? null : targetOf(part, event.clientX, event.clientY);
    changed();
  }

  function onLeave(): void {
    hover = null;
    changed();
  }

  function onFocusChange(): void {
    // The focus is between two parts while it moves; read it once it has settled.
    window.setTimeout(() => {
      focus = focusTarget();
      changed();
    }, 0);
  }

  document.addEventListener("pointermove", onMove, true);
  document.documentElement.addEventListener("mouseleave", onLeave);
  window.addEventListener("blur", onLeave);
  document.addEventListener("focusin", onFocusChange, true);
  document.addEventListener("focusout", onFocusChange, true);

  return {
    stop: () => {
      document.removeEventListener("pointermove", onMove, true);
      document.documentElement.removeEventListener("mouseleave", onLeave);
      window.removeEventListener("blur", onLeave);
      document.removeEventListener("focusin", onFocusChange, true);
      document.removeEventListener("focusout", onFocusChange, true);
      if (timer !== undefined) window.clearTimeout(timer);
    },
    refresh: () => changed(true),
  };
}
