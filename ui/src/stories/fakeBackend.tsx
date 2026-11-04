/**
 * A backend for the screens in Storybook. A screen reaches its backend only
 * through `lib/backend.ts`, so connecting it to this one draws the real screen
 * with no app behind it: each command is answered from a table, and `emit`
 * plays an event the app would send.
 */
import type { ReactElement } from "react";
import type { Decorator } from "@storybook/react-vite";
import { setBackend, type Backend } from "../lib/backend";

/** What a command answers: a value, or a function of its arguments. Throwing rejects. */
export type Answers = Record<string, unknown>;

export interface FakeBackend extends Backend {
  /** Play an event to whoever listens for it */
  emit(event: string, payload: unknown): void;
}

export function fakeBackend(answers: Answers): FakeBackend {
  const listeners = new Map<string, Set<(payload: never) => void>>();
  return {
    async invoke<T>(cmd: string, args?: unknown): Promise<T> {
      const answer = answers[cmd];
      if (typeof answer === "function") return (answer as (args?: unknown) => T)(args);
      return answer as T;
    },
    async listen<T>(event: string, handler: (payload: T) => void) {
      const set = listeners.get(event) ?? new Set();
      set.add(handler as (payload: never) => void);
      listeners.set(event, set);
      return () => set.delete(handler as (payload: never) => void);
    },
    emit(event, payload) {
      listeners.get(event)?.forEach((h) => h(payload as never));
    },
  };
}

/** Connects the screen to a fake backend before it mounts */
export function withBackend(answers: Answers): Decorator {
  return (Story): ReactElement => {
    setBackend(fakeBackend(answers));
    return <Story />;
  };
}
