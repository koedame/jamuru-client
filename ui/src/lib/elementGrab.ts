/**
 * Hold Alt+Shift, point at something on the screen and click: a description of
 * that element, the state it is in and the screen it is on goes to the
 * clipboard, to be pasted into a conversation with an LLM that is helping with
 * the app (`elementReport.ts` says what is in it).
 *
 * Only development and beta builds have this. `main.tsx` imports this module
 * behind a build-time flag, so a release build does not contain it at all.
 */

import { getCurrentWindow } from "@tauri-apps/api/window";
import i18n from "../i18n";
import { buildElementReport } from "./elementReport";
import { appEnvironment, sessionGet, settingsGet } from "./tauri";

const OVERLAY_ID = "jamuru-element-grab-overlay";
const TOAST_ID = "jamuru-element-grab-toast";
const TOAST_MS = 1500;

function isOwn(el: Element | null): boolean {
  return !!el && (el.id === OVERLAY_ID || el.id === TOAST_ID);
}

function legacyCopy(text: string): boolean {
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.cssText = "position:fixed;top:-1000px;left:-1000px;opacity:0";
  document.body.appendChild(area);
  const selection = document.getSelection();
  const previous = selection?.rangeCount ? selection.getRangeAt(0) : null;
  area.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  area.remove();
  if (previous && selection) {
    selection.removeAllRanges();
    selection.addRange(previous);
  }
  return ok;
}

async function copy(text: string): Promise<boolean> {
  if (navigator.clipboard && window.isSecureContext) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // A webview that refuses falls back to the selection-based copy.
    }
  }
  return legacyCopy(text);
}

function windowLabel(): string | undefined {
  try {
    return getCurrentWindow().label;
  } catch {
    return undefined;
  }
}

/** The result, or null when it cannot be read: one missing part must not cost the copy. */
async function readOrNull<T>(read: () => Promise<T>): Promise<T | null> {
  try {
    return await read();
  } catch {
    return null;
  }
}

function showToast(text: string): void {
  let toast = document.getElementById(TOAST_ID);
  if (!toast) {
    toast = document.createElement("div");
    toast.id = TOAST_ID;
    toast.style.cssText = [
      "position:fixed",
      "bottom:16px",
      "right:16px",
      "z-index:2147483647",
      "background:#222",
      "color:#fff",
      "padding:8px 12px",
      "border-radius:6px",
      "font:12px sans-serif",
      "pointer-events:none",
      "transition:opacity .2s",
    ].join(";");
    document.body.appendChild(toast);
  }
  toast.textContent = text;
  toast.style.opacity = "1";
  const shown = toast;
  window.setTimeout(() => {
    shown.style.opacity = "0";
  }, TOAST_MS);
}

/** Starts listening for the shortcut. Returns a function that stops it. */
export function installElementGrab(): () => void {
  let active = false;
  let hovered: Element | null = null;
  let pointer: { x: number; y: number } | null = null;
  let overlay: HTMLElement | null = null;

  // A box drawn over the element rather than a style on it: the page is left
  // as it is, so what is copied is what was on screen.
  function ensureOverlay(): HTMLElement {
    if (!overlay) {
      overlay = document.createElement("div");
      overlay.id = OVERLAY_ID;
      overlay.style.cssText = [
        "position:fixed",
        "z-index:2147483646",
        "pointer-events:none",
        "box-sizing:border-box",
        "border:2px solid #ff5252",
        "background:rgba(255,82,82,.12)",
      ].join(";");
      document.body.appendChild(overlay);
    }
    return overlay;
  }

  function draw(): void {
    if (!hovered || !hovered.isConnected) {
      overlay?.remove();
      overlay = null;
      return;
    }
    const rect = hovered.getBoundingClientRect();
    const box = ensureOverlay();
    box.style.left = `${rect.left}px`;
    box.style.top = `${rect.top}px`;
    box.style.width = `${rect.width}px`;
    box.style.height = `${rect.height}px`;
  }

  function pointAt(x: number, y: number): void {
    const el = document.elementFromPoint(x, y);
    hovered = isOwn(el) ? null : el;
    draw();
  }

  function swallow(e: Event): void {
    e.preventDefault();
    e.stopPropagation();
  }

  function onMove(e: MouseEvent): void {
    pointer = { x: e.clientX, y: e.clientY };
    if (active) pointAt(e.clientX, e.clientY);
  }

  async function onClick(e: MouseEvent): Promise<void> {
    swallow(e);
    const el = hovered;
    if (!el) return;
    const [app, audio, session] = await Promise.all([
      readOrNull(appEnvironment),
      readOrNull(settingsGet),
      readOrNull(sessionGet),
    ]);
    const ok = await copy(buildElementReport(el, { windowLabel: windowLabel(), app, audio, session }));
    showToast(i18n.t(ok ? "devtools.elementGrab.copied" : "devtools.elementGrab.failed"));
  }

  const swallowed = ["pointerdown", "pointerup", "mousedown", "mouseup", "dblclick", "contextmenu"];

  function activate(): void {
    if (active) return;
    active = true;
    if (pointer) pointAt(pointer.x, pointer.y);
    swallowed.forEach((type) => document.addEventListener(type, swallow, true));
    document.addEventListener("click", onClick, true);
    window.addEventListener("scroll", draw, true);
    window.addEventListener("resize", draw);
  }

  function deactivate(): void {
    if (!active) return;
    active = false;
    hovered = null;
    draw();
    swallowed.forEach((type) => document.removeEventListener(type, swallow, true));
    document.removeEventListener("click", onClick, true);
    window.removeEventListener("scroll", draw, true);
    window.removeEventListener("resize", draw);
  }

  function onKey(e: KeyboardEvent): void {
    if (e.altKey && e.shiftKey) activate();
    else deactivate();
  }

  document.addEventListener("mousemove", onMove, true);
  window.addEventListener("keydown", onKey);
  window.addEventListener("keyup", onKey);
  window.addEventListener("blur", deactivate);

  return () => {
    deactivate();
    document.removeEventListener("mousemove", onMove, true);
    window.removeEventListener("keydown", onKey);
    window.removeEventListener("keyup", onKey);
    window.removeEventListener("blur", deactivate);
  };
}
