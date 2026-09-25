/**
 * Hook for listening to Tauri window events
 *
 * Provides unified access to session state change events from the Rust backend.
 */

import { useEffect, useCallback, useState } from "react";
import { listenEvent } from "../lib/backend";

type UnlistenFn = () => void;

/**
 * Session event types
 */
export type SessionEvent =
  | { type: "connected" }
  | { type: "disconnected"; reason?: string };

/**
 * Hook to listen for session state changes
 *
 * @param onConnect Callback when session connects
 * @param onDisconnect Callback when session disconnects
 * @returns Object with isInSession state
 *
 * @example
 * ```tsx
 * function MyComponent() {
 *   const { isInSession } = useSessionEvents(
 *     () => console.log("Connected!"),
 *     (reason) => console.log("Disconnected:", reason)
 *   );
 *
 *   return <div>{isInSession ? "In Session" : "Not Connected"}</div>;
 * }
 * ```
 */
export function useSessionEvents(
  onConnect?: () => void,
  onDisconnect?: (reason?: string) => void
) {
  const [isInSession, setIsInSession] = useState(false);

  useEffect(() => {
    const unlistenFns: UnlistenFn[] = [];

    // Listen for connection event
    listenEvent<void>("session:connected", () => {
      setIsInSession(true);
      onConnect?.();
    }).then((unlisten) => unlistenFns.push(unlisten));

    // Listen for disconnection event
    listenEvent<string>("session:disconnected", (reason) => {
      setIsInSession(false);
      onDisconnect?.(reason);
    }).then((unlisten) => unlistenFns.push(unlisten));

    // Cleanup listeners on unmount
    return () => {
      unlistenFns.forEach((unlisten) => unlisten());
    };
  }, [onConnect, onDisconnect]);

  return { isInSession };
}

/**
 * Hook to listen for any window event
 *
 * @param eventName The event name to listen for
 * @param handler Event handler callback
 *
 * @example
 * ```tsx
 * function MyComponent() {
 *   useWindowEvent<{ volume: number }>("mixer:volume-changed", (payload) => {
 *     console.log("Volume changed to:", payload.volume);
 *   });
 * }
 * ```
 */
export function useWindowEvent<T = unknown>(
  eventName: string,
  handler: (payload: T) => void
) {
  useEffect(() => {
    let unlisten: UnlistenFn | null = null;

    listenEvent<T>(eventName, handler).then((fn) => {
      unlisten = fn;
    });

    return () => {
      unlisten?.();
    };
  }, [eventName, handler]);
}

/**
 * Hook to emit events to other windows
 *
 * @returns emit function
 *
 * @example
 * ```tsx
 * function MyComponent() {
 *   const emit = useWindowEmit();
 *
 *   const handleClick = async () => {
 *     await emit("my-event", { data: "hello" });
 *   };
 * }
 * ```
 */
export function useWindowEmit() {
  const emit = useCallback(
    async <T = unknown>(eventName: string, payload?: T) => {
      // Dynamic import to avoid issues in non-Tauri environments
      const { emit: tauriEmit } = await import("@tauri-apps/api/event");
      await tauriEmit(eventName, payload);
    },
    []
  );

  return emit;
}

/**
 * Hook to emit events to a specific window
 *
 * @returns emitTo function
 *
 * @example
 * ```tsx
 * function MyComponent() {
 *   const emitTo = useWindowEmitTo();
 *
 *   const notifyMixer = async () => {
 *     await emitTo("mixer", "volume-update", { channel: 0, volume: 80 });
 *   };
 * }
 * ```
 */
export function useWindowEmitTo() {
  const emitTo = useCallback(
    async <T = unknown>(target: string, eventName: string, payload?: T) => {
      const { emitTo: tauriEmitTo } = await import("@tauri-apps/api/event");
      await tauriEmitTo(target, eventName, payload);
    },
    []
  );

  return emitTo;
}
