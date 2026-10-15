/**
 * How long until a help that sits idle ends by itself (ADR-044 §5), counted
 * down on the screen between the backend's announcements. The helped app
 * announces the time left whenever something is done and every few seconds, so
 * a screen that opened late catches up and a drift in the count is corrected.
 */
import { useCallback, useEffect, useState } from "react";
import { useWindowEvent } from "../../hooks/useWindowEvents";
import { HELP_REMAINING_EVENT } from "../../lib/tauri";

/** `m:ss` for a number of milliseconds, rounded up so it reads 0:00 only when it has run out */
export function formatRemaining(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

interface Announced {
  ms: number;
  at: number;
}

/**
 * The milliseconds left while `active`, or null until they are known (and
 * whenever not active). `seed` sets them from a read of the record.
 */
export function useHelpRemaining(active: boolean): { ms: number | null; seed: (ms: number | null) => void } {
  const [announced, setAnnounced] = useState<Announced | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const onAnnounced = useCallback((payload: { remaining_ms: number }) => {
    setAnnounced({ ms: payload.remaining_ms, at: Date.now() });
  }, []);
  useWindowEvent<{ remaining_ms: number }>(HELP_REMAINING_EVENT, onAnnounced);

  const seed = useCallback((ms: number | null) => {
    setAnnounced(ms === null ? null : { ms, at: Date.now() });
  }, []);

  const counting = active && announced !== null;
  useEffect(() => {
    if (!counting) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [counting]);

  if (!active || announced === null) return { ms: null, seed };
  return { ms: Math.max(0, announced.ms - (now - announced.at)), seed };
}
