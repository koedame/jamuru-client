/**
 * RecordingPanel - Record the room (ADR-095)
 *
 * One card in the room sidebar under the master meter, the thing it records:
 * a title with the time on top, then either the button that starts a
 * recording (with the choice of the microphone in a file of its own) or, while
 * it runs, the file's name and the button that stops it. When a recording has
 * ended, one line says how and where it was saved.
 */

import { useTranslation } from "react-i18next";
import type { RecordingEnded, RecordingEndedBy } from "../../lib/tauri";
import "./RecordingPanel.css";

export interface RecordingPanelProps {
  /** Whether a recording runs */
  recording: boolean;
  /** Seconds recorded so far */
  seconds: number;
  /** The running recording's file */
  fileName: string | null;
  /** Whether the next recording also takes the microphone, in a file of its own */
  withSelf: boolean;
  /** The latest recording that ended, until the next starts */
  last: RecordingEnded | null;
  /** What went wrong the last time a start or stop was asked for */
  error?: string | null;
  /** A start or stop is on its way */
  busy?: boolean;
  onWithSelfChange: (withSelf: boolean) => void;
  onStart: () => void;
  onStop: () => void;
  onOpenFolder: () => void;
}

/** `75` → `1:15`, `3725` → `1:02:05` */
export function formatDuration(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  const pair = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pair(m)}:${pair(s)}` : `${m}:${pair(s)}`;
}

/** The text that says how a recording came to an end */
export function endedKey(by: RecordingEndedBy) {
  return {
    stopped: "recording.ended.stopped",
    session_ended: "recording.ended.sessionEnded",
    size_limit: "recording.ended.sizeLimit",
    write_failed: "recording.ended.writeFailed",
  }[by];
}

/** Beside a participant's name: they record the room */
export function RecordingMark() {
  const { t } = useTranslation();
  return (
    <span className="recording-mark" data-testid="recording-mark">
      <span className="recording-mark__dot" aria-hidden="true" />
      {t("recording.on")}
    </span>
  );
}

export function RecordingPanel({
  recording,
  seconds,
  fileName,
  withSelf,
  last,
  error = null,
  busy = false,
  onWithSelfChange,
  onStart,
  onStop,
  onOpenFolder,
}: RecordingPanelProps) {
  const { t } = useTranslation();

  return (
    <div
      className={`recording-panel ${recording ? "recording-panel--on" : ""}`}
      data-testid="recording-panel"
      data-recording={recording}
    >
      <div className="recording-panel__header">
        <span className="recording-panel__title">
          {recording && <span className="recording-panel__dot" aria-hidden="true" />}
          {recording ? t("recording.on") : t("recording.title")}
        </span>
        {recording && (
          <span className="recording-panel__time" data-testid="recording-time">
            {formatDuration(seconds)}
          </span>
        )}
      </div>

      {recording ? (
        <>
          {fileName && (
            <span className="recording-panel__file" title={fileName}>
              {fileName}
            </span>
          )}
          <button
            type="button"
            className="recording-panel__button recording-panel__button--stop"
            data-testid="recording-stop"
            disabled={busy}
            onClick={onStop}
          >
            {t("recording.stop")}
          </button>
        </>
      ) : (
        <>
          <label className="recording-panel__option">
            <input
              type="checkbox"
              checked={withSelf}
              onChange={(e) => onWithSelfChange(e.target.checked)}
            />
            <span>{t("recording.withSelf")}</span>
          </label>
          <button
            type="button"
            className="recording-panel__button"
            data-testid="recording-start"
            disabled={busy}
            onClick={onStart}
          >
            {t("recording.start")}
          </button>
          <span className="recording-panel__note">{t("recording.notice")}</span>
        </>
      )}

      {error && (
        <span className="recording-panel__error" role="alert" data-testid="recording-error">
          {error}
        </span>
      )}

      {!recording && last && <RecordingResult last={last} onOpenFolder={onOpenFolder} />}
    </div>
  );
}

function RecordingResult({
  last,
  onOpenFolder,
}: {
  last: RecordingEnded;
  onOpenFolder: () => void;
}) {
  const { t } = useTranslation();
  const why = endedKey(last.ended_by);
  const failed = last.ended_by === "write_failed";

  return (
    <div className="recording-panel__result" data-testid="recording-result">
      <span className={failed ? "recording-panel__error" : "recording-panel__note"}>
        {t(why, { time: formatDuration(last.seconds) })}
      </span>
      <span className="recording-panel__file" title={last.file_name}>
        {last.file_name}
      </span>
      {last.dropped_frames > 0 && (
        <span className="recording-panel__error" data-testid="recording-dropped">
          {t("recording.ended.dropped")}
        </span>
      )}
      <button
        type="button"
        className="recording-panel__link"
        data-testid="recording-open-folder"
        onClick={onOpenFolder}
      >
        {t("recording.openFolder")}
      </button>
    </div>
  );
}

export default RecordingPanel;
