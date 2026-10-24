import { useTranslation } from "react-i18next";
import "./StartupUpdate.css";

/** How far the update has come (the same shape the backend reports) */
export type StartupUpdatePhase =
  | { phase: "checking" }
  /** `total` is `null` when how big the download is is not known */
  | { phase: "downloading"; received: number; total: number | null }
  | { phase: "installing" }
  | { phase: "restarting" };

export interface StartupUpdateProps {
  progress: StartupUpdatePhase;
  /** Opens the app without waiting. Offered only while checking or downloading */
  onSkip: () => void;
  /** True once the person has asked to open without waiting */
  skipping?: boolean;
}

/** Whole percent of the download, or `null` when it cannot be told */
function percentOf(progress: StartupUpdatePhase): number | null {
  if (progress.phase !== "downloading" || !progress.total) return null;
  return Math.min(100, Math.floor((progress.received / progress.total) * 100));
}

/**
 * The small window a start shows while it updates the app before opening it
 * (ADR-089): what it is doing, how far the download is, and a way to open the
 * app without waiting until the install begins.
 */
export function StartupUpdate({ progress, onSkip, skipping = false }: StartupUpdateProps) {
  const { t } = useTranslation();
  const percent = percentOf(progress);
  const canSkip = progress.phase === "checking" || progress.phase === "downloading";

  return (
    <main className="startup-update" data-testid="startup-update" data-state={progress.phase}>
      <p className="startup-update__status" role="status">
        {t(`startupUpdate.${progress.phase}`)}
        {percent !== null && (
          <span className="startup-update__percent">{t("startupUpdate.percent", { percent })}</span>
        )}
      </p>
      <div
        className={`startup-update__bar${percent === null ? " startup-update__bar--unknown" : ""}`}
        role="progressbar"
        aria-label={t("startupUpdate.progressLabel")}
        aria-valuemin={percent === null ? undefined : 0}
        aria-valuemax={percent === null ? undefined : 100}
        aria-valuenow={percent ?? undefined}
        data-testid="startup-update-bar"
      >
        <div
          className="startup-update__fill"
          style={percent === null ? undefined : { width: `${percent}%` }}
        />
      </div>
      <p className="startup-update__note">{t("startupUpdate.note")}</p>
      {canSkip && (
        <button
          type="button"
          className="startup-update__skip"
          onClick={onSkip}
          disabled={skipping}
          data-testid="startup-update-skip"
        >
          {t("startupUpdate.skip")}
        </button>
      )}
    </main>
  );
}
