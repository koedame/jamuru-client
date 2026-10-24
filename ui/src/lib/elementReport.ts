/**
 * What to tell an LLM about one element of the screen (development and beta
 * builds only, see `elementGrab.ts`).
 *
 * The report is written to be pasted into a conversation: it names the element
 * the person pointed at, the state it was in, the screen it was on, and enough
 * of the source side (test id, class names, React components) to find the code
 * with one search. It is plain English labels so it reads the same whatever the
 * UI language is.
 */

import { cssPath, isVisible } from "./domTarget";
import { HELP_HASH } from "./helperBackend";
import type { AppEnvironment, AudioSettings, SessionSnapshot } from "./tauri";

const MAX_TEXT = 200;
const MAX_HTML = 1500;
const MAX_ANCESTORS = 12;
const MAX_COMPONENTS = 4;
const MAX_PROP_VALUE = 80;

/** ARIA attributes that say what state a control is in. */
const STATE_ATTRIBUTES = [
  "aria-pressed",
  "aria-selected",
  "aria-checked",
  "aria-expanded",
  "aria-disabled",
  "aria-invalid",
  "aria-busy",
  "aria-current",
  "aria-valuenow",
  "aria-valuetext",
  "data-state",
];

/**
 * What the report says besides the element. Each part is null when it could
 * not be read (a window that cannot call that command), and the report says so.
 */
export interface ReportContext {
  /** The Tauri window's label, when known */
  windowLabel?: string;
  app?: AppEnvironment | null;
  audio?: AudioSettings | null;
  session?: SessionSnapshot | null;
}

function squash(text: string, max: number): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max)}… (${flat.length} chars)` : flat;
}

/** `tag.class-a.class-b[data-testid=…][data-state=…]`: one element in a line. */
function brief(el: Element): string {
  const classes = [...el.classList].map((c) => `.${c}`).join("");
  const testId = el.getAttribute("data-testid");
  const state = el.getAttribute("data-state");
  return (
    el.tagName.toLowerCase() +
    classes +
    (testId ? `[data-testid="${testId}"]` : "") +
    (state ? `[data-state="${state}"]` : "")
  );
}

function controlValue(el: Element): string | null {
  if (el instanceof HTMLInputElement) {
    // A password is not the LLM's business, and not worth a leak into a chat.
    if (el.type === "password") return null;
    if (el.type === "checkbox" || el.type === "radio") return null;
    return el.value;
  }
  if (el instanceof HTMLTextAreaElement) return el.value;
  if (el instanceof HTMLSelectElement) {
    const option = el.selectedOptions[0];
    return option ? `${el.value} (${squash(option.text, MAX_TEXT)})` : el.value;
  }
  return null;
}

/** The element's HTML, without the value of any password field in it. */
function htmlWithoutSecrets(el: Element): string {
  const copy = el.cloneNode(true) as Element;
  const fields = [copy, ...copy.querySelectorAll("input")];
  for (const field of fields) {
    if (field instanceof HTMLInputElement && field.type === "password") field.removeAttribute("value");
  }
  return copy.outerHTML;
}

function stateLines(el: Element): string[] {
  const lines: string[] = [];
  lines.push(`visible: ${isVisible(el)}`);
  if ("disabled" in el) lines.push(`disabled: ${(el as HTMLButtonElement).disabled}`);
  if (el instanceof HTMLInputElement && (el.type === "checkbox" || el.type === "radio")) {
    lines.push(`checked: ${el.checked}`);
  }
  const value = controlValue(el);
  if (value !== null) lines.push(`value: ${JSON.stringify(value)}`);
  lines.push(`focused: ${el === document.activeElement}`);
  for (const name of STATE_ATTRIBUTES) {
    const v = el.getAttribute(name);
    if (v !== null) lines.push(`${name}: ${v}`);
  }
  const rect = el.getBoundingClientRect();
  lines.push(
    `rect: x=${Math.round(rect.x)} y=${Math.round(rect.y)} ` +
      `w=${Math.round(rect.width)} h=${Math.round(rect.height)}`
  );
  return lines;
}

interface Fiber {
  type: unknown;
  return: Fiber | null;
  memoizedProps: Record<string, unknown> | null;
}

function fiberOf(el: Element): Fiber | null {
  const key = Object.keys(el).find((k) => k.startsWith("__reactFiber$"));
  return key ? ((el as unknown as Record<string, Fiber>)[key] ?? null) : null;
}

function componentName(type: unknown): string | null {
  if (typeof type === "function") {
    const named = type as { displayName?: string; name?: string };
    return named.displayName || named.name || null;
  }
  if (type && typeof type === "object") {
    const wrapped = type as {
      displayName?: string;
      render?: { name?: string };
      type?: { name?: string };
    };
    return wrapped.displayName || wrapped.render?.name || wrapped.type?.name || null;
  }
  return null;
}

/** Props that are plain values: the state a component was given, without callbacks or trees. */
function primitiveProps(props: Record<string, unknown> | null): string {
  if (!props) return "";
  const shown: string[] = [];
  for (const [key, value] of Object.entries(props)) {
    if (key === "children") continue;
    if (typeof value === "string") {
      shown.push(`${key}=${JSON.stringify(squash(value, MAX_PROP_VALUE))}`);
    } else if (typeof value === "number" || typeof value === "boolean") {
      shown.push(`${key}=${value}`);
    }
  }
  return shown.join(" ");
}

/**
 * The React components that rendered the element, nearest first, with their
 * plain props. A beta build is minified, so a name can be a letter or two;
 * those are left out because searching for them finds nothing.
 */
function componentLines(el: Element): string[] {
  const lines: string[] = [];
  for (let fiber = fiberOf(el); fiber && lines.length < MAX_COMPONENTS; fiber = fiber.return) {
    const name = componentName(fiber.type);
    if (!name || name.length < 3) continue;
    const props = primitiveProps(fiber.memoizedProps);
    lines.push(props ? `${name} ${props}` : name);
  }
  return lines;
}

function ancestorLines(el: Element): string[] {
  const lines: string[] = [];
  for (
    let node = el.parentElement;
    node && node !== document.documentElement && lines.length < MAX_ANCESTORS;
    node = node.parentElement
  ) {
    if (node.classList.length === 0 && !node.id && !node.hasAttribute("data-testid")) continue;
    lines.push(node.id ? `${brief(node)}#${node.id}` : brief(node));
  }
  return lines;
}

function screenName(): string {
  const hash = window.location.hash;
  if (hash === HELP_HASH) return "help (a window helping another participant)";
  if (hash === "#/settings" || hash === "#settings") return "settings";
  return "main";
}

function deviceName(devices: AudioSettings["input_devices"], id: string | null): string {
  if (id === null) return "system default";
  const device = devices.find((d) => d.id === id);
  return device ? `${device.name}${device.is_bluetooth ? " (bluetooth)" : ""}` : "not found";
}

function appLines(app: AppEnvironment | null | undefined): string[] {
  if (!app) return ["unavailable in this window"];
  return [
    `app version: ${app.app_version}`,
    `build: ${app.build}`,
    `os: ${[app.os, app.os_version].filter(Boolean).join(" ")} (${app.arch})`,
    ...(app.ram_gb !== null ? [`ram: ${app.ram_gb} GB`] : []),
    `audio host: ${app.audio_host}`,
  ];
}

function audioLines(audio: AudioSettings | null | undefined): string[] {
  if (!audio) return ["unavailable"];
  const pair = (p: AudioSettings["input_channels"]) =>
    p.right === null ? `${p.left} (mono)` : `${p.left}/${p.right}`;
  return [
    `input device: ${deviceName(audio.input_devices, audio.input_device_id)}`,
    `input channels: ${pair(audio.input_channels)} of ${audio.input_channel_count ?? "?"}`,
    `output device: ${deviceName(audio.output_devices, audio.output_device_id)}`,
    `output channels: ${pair(audio.output_channels)} of ${audio.output_channel_count ?? "?"}`,
    `transmit channels: ${audio.transmit_channels}`,
    `buffer size: ${audio.buffer_size} frames`,
    `sample rate: ${audio.sample_rate} Hz`,
    ...(audio.wasapi ? [`wasapi mode: ${audio.wasapi.mode}${audio.wasapi.advice.applies ? "" : " (not used: asio)"}`] : []),
  ];
}

function sessionLines(session: SessionSnapshot | null | undefined): string[] {
  if (!session) return ["unavailable"];
  return [
    `phase: ${session.phase}`,
    ...(session.error ? [`error: ${squash(session.error, MAX_TEXT)}`] : []),
    `in a room: ${session.room !== null}`,
    ...(session.room ? [`participants: ${session.room.participants.length}`] : []),
    `audio links: ${session.streaming_peer_ids.length}`,
    `signaling reconnect: ${session.signaling_reconnect}`,
  ];
}

function list(title: string, lines: string[]): string[] {
  return lines.length ? [`${title}:`, ...lines.map((l) => `  ${l}`)] : [];
}

export function buildElementReport(el: Element, context: ReportContext = {}): string {
  const attrs = ["id", "role", "aria-label", "title", "name", "type", "placeholder"]
    .filter((a) => el.hasAttribute(a))
    .map((a) => `${a}: ${JSON.stringify(el.getAttribute(a))}`);
  const testId = el.getAttribute("data-testid");
  const html = squash(htmlWithoutSecrets(el), MAX_HTML);

  return [
    "## Element",
    `selector: ${cssPath(el)}`,
    `element: ${brief(el)}`,
    ...(testId ? [`data-testid: ${testId}`] : []),
    ...attrs,
    `text: ${JSON.stringify(squash(el.textContent ?? "", MAX_TEXT))}`,
    "",
    "## State when copied",
    ...stateLines(el),
    "",
    "## Where",
    `screen: ${screenName()}`,
    ...(context.windowLabel ? [`window: ${context.windowLabel}`] : []),
    `page title: ${document.title}`,
    `url: ${window.location.href}`,
    `language: ${document.documentElement.lang || navigator.language}`,
    `viewport: ${window.innerWidth}x${window.innerHeight}`,
    `time: ${new Date().toISOString()}`,
    ...list("inside (nearest first)", ancestorLines(el)),
    "",
    "## App",
    ...appLines(context.app),
    "",
    // In the window that helps someone, the backend is the helped person's app.
    `## Audio settings${window.location.hash === HELP_HASH ? " (of the helped participant's app)" : ""}`,
    ...audioLines(context.audio),
    "",
    "## Session",
    ...sessionLines(context.session),
    "",
    "## Source hints",
    ...list("react components (nearest first)", componentLines(el)),
    "html:",
    `  ${html}`,
  ].join("\n");
}
