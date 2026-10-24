/**
 * StartupUpdateScreen - the small window a start shows while it updates the
 * app before opening it (ADR-088). It reads how far the update is, follows it
 * as the backend reports it, and passes on the person's wish to open the app
 * without waiting. The backend closes the window when the app opens.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { StartupUpdate } from "../components/StartupUpdate";
import { useWindowEvent } from "../hooks/useWindowEvents";
import {
  UPDATE_STARTUP,
  updateStartupProgress,
  updateStartupSkip,
  type StartupProgress,
} from "../lib/tauri";

/** The URL hash of the updating window (`updater/startup.rs` opens it) */
export const UPDATING_HASH = "#/updating";

export function StartupUpdateScreen() {
  const [progress, setProgress] = useState<StartupProgress>({ phase: "checking" });
  const [skipping, setSkipping] = useState(false);
  // What is heard is newer than what a read that was answered later found.
  const heard = useRef(false);

  const onProgress = useCallback((next: StartupProgress) => {
    heard.current = true;
    setProgress(next);
  }, []);
  useWindowEvent<StartupProgress>(UPDATE_STARTUP, onProgress);

  useEffect(() => {
    updateStartupProgress()
      .then((now) => {
        if (now !== null && !heard.current) setProgress(now);
      })
      .catch((e) => console.error("Could not read how far the update is:", e));
  }, []);

  const skip = useCallback(() => {
    setSkipping(true);
    updateStartupSkip().catch((e) => {
      console.error("Could not open the app without waiting:", e);
      setSkipping(false);
    });
  }, []);

  return <StartupUpdate progress={progress} onSkip={skip} skipping={skipping} />;
}
