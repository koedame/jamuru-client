/**
 * Recording the room, as the main window has it (ADR-095): what the backend
 * says about it (read while a session is up, and once more when it ends, to
 * learn how it ended), the buttons' effects, the participants who record, and
 * the notice for a recording that ended without being asked to.
 */

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  recordingOpenFolder,
  recordingStart,
  recordingStatus,
  recordingStop,
  type RecordingStatus,
} from "../../lib/tauri";
import { Toast } from "../Toast";
import { endedKey } from "./RecordingPanel";

/** How often the time and who records are read while a session is up */
export const RECORDING_POLL_MS = 500;
/** How long the notice of an ended recording stays */
export const ENDED_NOTICE_MS = 10_000;

export interface RoomRecording {
  status: RecordingStatus;
  withSelf: boolean;
  setWithSelf: (withSelf: boolean) => void;
  busy: boolean;
  /** What went wrong the last time a start or stop was asked for */
  error: string | null;
  start: () => void;
  stop: () => void;
  openFolder: () => void;
  /** The passing notice that a recording ended by itself, for the toast stack */
  toast: ReactNode;
}

const IDLE: RecordingStatus = {
  recording: false,
  seconds: 0,
  dropped_frames: 0,
  file_name: null,
  last: null,
  peers: [],
};

/**
 * `active`: a session is up (the room is entered). Not for a helper's window,
 * which does not record the helped app's room.
 */
export function useRoomRecording(active: boolean): RoomRecording {
  const { t } = useTranslation();
  const [status, setStatus] = useState<RecordingStatus>(IDLE);
  const [withSelf, setWithSelf] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // The last ending that has been seen, so each shows once and one from
  // before this window opened does not show
  const seen = useRef<number | null>(null);

  const read = useCallback(async () => {
    try {
      const next = await recordingStatus();
      setStatus(next);
      const last = next.last;
      if (seen.current === null) {
        seen.current = last?.id ?? 0;
      } else if (last && last.id > seen.current) {
        seen.current = last.id;
        // The person who stopped it is looking at the panel that says so
        if (last.ended_by !== "stopped") {
          setNotice(t("recording.endedNotice", { why: t(endedKey(last.ended_by)), file: last.file_name }));
        }
      }
    } catch (e) {
      console.error("Failed to get the recording status:", e);
    }
  }, [t]);

  // Read while a session is up, and once on leaving it (its recording ends then)
  const wasActive = useRef(false);
  useEffect(() => {
    if (!active) {
      if (wasActive.current) {
        wasActive.current = false;
        void read();
      }
      return;
    }
    wasActive.current = true;
    void read();
    const interval = setInterval(() => void read(), RECORDING_POLL_MS);
    return () => clearInterval(interval);
  }, [active, read]);

  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(null), ENDED_NOTICE_MS);
    return () => clearTimeout(timer);
  }, [notice]);

  const run = useCallback(async (ask: () => Promise<RecordingStatus>) => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await ask());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const start = useCallback(() => void run(() => recordingStart(withSelf)), [run, withSelf]);
  const stop = useCallback(() => void run(recordingStop), [run]);
  const openFolder = useCallback(() => {
    recordingOpenFolder().catch((e) => setError(String(e)));
  }, []);

  return {
    status,
    withSelf,
    setWithSelf,
    busy,
    error,
    start,
    stop,
    openFolder,
    toast: notice ? <Toast type="warning" message={notice} /> : null,
  };
}
