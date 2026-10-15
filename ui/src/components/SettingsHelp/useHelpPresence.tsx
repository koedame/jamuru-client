/**
 * Where the two people of a settings help are looking (ADR-044 §5,
 * REQ-RMT-034): sends where this screen's person is pointing and focusing,
 * hears where the other person is, and gives the frames to draw for it.
 *
 * Both windows use it. The helper's window sends through the help portal
 * (`help_presence`, which the helped app may refuse like any call) and hears the
 * helped person's presence as an event the portal carries; the helped person's
 * screen publishes its own (`help_presence_publish`) and hears the helper's.
 * Nothing runs while `enabled` is false, so no help means no listening and no
 * sending.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { listenEvent } from "../../lib/backend";
import { findPresenceElement, watchPresence } from "../../lib/presence";
import {
  HELP_PRESENCE_FROM_HELPED,
  HELP_PRESENCE_FROM_HELPER,
  helpPresence,
  helpPresencePublish,
  type Presence,
  type PresenceTarget,
} from "../../lib/tauri";
import { PresenceMarks, type PresenceMark } from "./PresenceMarks";

/** How often the frames are put back where their parts are (a part can move under them) */
const RELAYOUT_MS = 500;

export interface UseHelpPresenceOptions {
  /** Which window this is: the helper's sends `help_presence` and hears the helped person's presence */
  role: "helper" | "helped";
  /** False when no help goes on: nothing is listened to or sent */
  enabled: boolean;
  /** The helper's settings panel is open (the helper's message says so) */
  panelOpen?: boolean;
  /** Whose frames these are */
  peerName: string;
}

export interface HelpPresence {
  /** The frames to draw over the screen */
  marks: React.ReactNode;
  /** Whether the last thing this window sent reached the other screen (the helper's window only knows) */
  delivered: boolean;
  /** Whether the other person has the helper's settings panel open (the helped person's screen) */
  remotePanelOpen: boolean;
}

function isTarget(value: unknown): value is PresenceTarget {
  const target = value as PresenceTarget | null;
  return typeof target?.id === "string" && typeof target.x === "number" && typeof target.y === "number";
}

/** What was heard, if it is a presence; anything else is nothing to show. */
function asPresence(value: unknown): Presence | null {
  if (typeof value !== "object" || value === null) return null;
  const { hover, focus, panel } = value as Record<string, unknown>;
  return {
    hover: isTarget(hover) ? hover : null,
    focus: isTarget(focus) ? focus : null,
    ...(typeof panel === "boolean" ? { panel } : {}),
  };
}

function marksFor(remote: Presence | null): PresenceMark[] {
  const marks: PresenceMark[] = [];
  const place = (kind: PresenceMark["kind"], target: PresenceTarget | null) => {
    const part = target && findPresenceElement(target.id);
    if (!target || !part) return;
    const rect = part.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;
    marks.push({
      kind,
      left: rect.left,
      top: rect.top,
      width: rect.width,
      height: rect.height,
      ...(kind === "hover" ? { at: { x: target.x, y: target.y } } : {}),
    });
  };
  place("hover", remote?.hover ?? null);
  place("focus", remote?.focus ?? null);
  return marks;
}

export function useHelpPresence({ role, enabled, panelOpen = false, peerName }: UseHelpPresenceOptions): HelpPresence {
  const [remote, setRemote] = useState<Presence | null>(null);
  const [delivered, setDelivered] = useState(false);
  const [marks, setMarks] = useState<PresenceMark[]>([]);

  const panelRef = useRef(panelOpen);
  panelRef.current = panelOpen;

  // Hearing the other person.
  useEffect(() => {
    if (!enabled) {
      setRemote(null);
      return;
    }
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    const event = role === "helper" ? HELP_PRESENCE_FROM_HELPED : HELP_PRESENCE_FROM_HELPER;
    listenEvent<unknown>(event, (payload) => setRemote(asPresence(payload)))
      .then((stop) => (cancelled ? stop() : (unlisten = stop)))
      .catch((e) => console.error("Could not listen for where the other person is looking:", e));
    return () => {
      cancelled = true;
      unlisten?.();
      setRemote(null);
    };
  }, [enabled, role]);

  // Telling the other person.
  const watching = useRef<ReturnType<typeof watchPresence> | null>(null);
  useEffect(() => {
    if (!enabled) return;
    // One line in the log when sending starts failing, not one for each of the ten a second.
    let failing = false;
    const send = (presence: Presence) => {
      const sent =
        role === "helper"
          ? helpPresence(presence).then((ack) => ack.delivered)
          : helpPresencePublish(presence).then(() => true);
      sent
        .then((reached) => {
          failing = false;
          setDelivered(reached);
        })
        .catch((e) => {
          if (!failing) console.warn("Could not tell the other person where this window is looking:", e);
          failing = true;
          setDelivered(false);
        });
    };
    const watch = watchPresence(send, () => (role === "helper" ? { panel: panelRef.current } : {}));
    watching.current = watch;
    // Say where things stand at the start, so the other side has a picture to begin with.
    watch.refresh();
    return () => {
      watching.current = null;
      watch.stop();
      setDelivered(false);
    };
  }, [enabled, role]);

  // The helper's panel opening or closing is said at once, not at the next movement.
  useEffect(() => {
    if (role === "helper") watching.current?.refresh();
  }, [role, panelOpen]);

  // Where the frames go is where the other person's parts are on this screen now.
  const relayout = useCallback(() => setMarks(marksFor(remote)), [remote]);
  useEffect(() => {
    relayout();
    if (remote === null) return;
    const timer = window.setInterval(relayout, RELAYOUT_MS);
    window.addEventListener("resize", relayout);
    window.addEventListener("scroll", relayout, true);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("resize", relayout);
      window.removeEventListener("scroll", relayout, true);
    };
  }, [remote, relayout]);

  return {
    marks: <PresenceMarks marks={marks} name={peerName} />,
    delivered,
    remotePanelOpen: remote?.panel === true,
  };
}
