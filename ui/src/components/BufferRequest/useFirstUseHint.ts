/**
 * A hint shown the first time a feature can be used, for a few seconds, and
 * never again once it has been shown or dismissed. "Shown" is kept on this
 * machine, so a hint is not repeated at every start.
 */
import { useCallback, useEffect, useState } from "react";

/** How long the hint stays up by itself */
export const HINT_MS = 20_000;

function alreadyShown(key: string): boolean {
  try {
    return localStorage.getItem(key) !== null;
  } catch {
    // No storage: the hint is shown every time rather than never.
    return false;
  }
}

function markShown(key: string): void {
  try {
    localStorage.setItem(key, "1");
  } catch {
    // Not kept; the hint shows again next time.
  }
}

/** `enabled` is whether the feature can be used here at all; a hint for a feature that cannot is not spent */
export function useFirstUseHint(key: string, enabled = true): { visible: boolean; dismiss: () => void } {
  const [visible, setVisible] = useState(() => enabled && !alreadyShown(key));

  useEffect(() => {
    if (!visible) return;
    markShown(key);
    const timer = setTimeout(() => setVisible(false), HINT_MS);
    return () => clearTimeout(timer);
  }, [key, visible]);

  const dismiss = useCallback(() => setVisible(false), []);
  return { visible, dismiss };
}
