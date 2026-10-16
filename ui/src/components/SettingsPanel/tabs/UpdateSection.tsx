/**
 * UpdateSection - The running version and the update check, on the About tab
 *
 * The person asks for the check and for the install (ADR-082); the background
 * update (ADR-041) goes on regardless. What it can do depends on the build: a
 * development or self-built build checks nothing, a package that cannot
 * replace itself (a Linux `.deb`, a Windows `.msi`) only says that a newer
 * release exists, and the others also install it and restart.
 */

import { useTranslation } from "react-i18next";
import type { UpdateSupport } from "../../../lib/tauri";
import "./UpdateSection.css";

export type UpdatePhase =
  | "idle"
  | "checking"
  | "up_to_date"
  | "available"
  | "installing"
  | "busy"
  | "error";

export interface UpdateSectionProps {
  /** The running version; null until it is read */
  current: string | null;
  /** What this build can do about a newer release; null until it is read */
  support: UpdateSupport | null;
  phase: UpdatePhase;
  /** The newer release, while `phase` is `available` or `installing` */
  version?: string | null;
  /** Why the check or the install failed, while `phase` is `error` */
  error?: string | null;
  /** The person is in a session: an install would cut the audio */
  inSession?: boolean;
  onCheck: () => void;
  onInstall: () => void;
}

export function UpdateSection({
  current,
  support,
  phase,
  version,
  error,
  inSession = false,
  onCheck,
  onInstall,
}: UpdateSectionProps) {
  const { t } = useTranslation();
  const working = phase === "checking" || phase === "installing";
  // The install button stays through the install and after it fails or is
  // refused, so it can be pressed again once the reason is gone.
  const canInstall =
    support === "install" &&
    Boolean(version) &&
    (phase === "available" || phase === "installing" || phase === "busy" || phase === "error");

  const message = (() => {
    if (support === "none") return t("settings.update.unsupported");
    switch (phase) {
      case "checking":
        return t("settings.update.checking");
      case "up_to_date":
        return t("settings.update.upToDate");
      case "available":
        return support === "install"
          ? t("settings.update.available", { version })
          : t("notification.newerRelease", { version });
      case "installing":
        return t("settings.update.installing", { version });
      case "busy":
        return t("settings.update.busy");
      case "error":
        return t("settings.update.failed", { message: error });
      default:
        return null;
    }
  })();

  return (
    <section className="update" data-testid="settings-update">
      <h2 className="update__title">{t("settings.update.title")}</h2>
      {current && (
        <p className="update__version" data-testid="settings-update-version">
          {t("settings.update.current", { version: current })}
        </p>
      )}
      {message && (
        <p
          className={`update__message${phase === "error" ? " update__message--error" : ""}`}
          role="status"
          data-testid="settings-update-message"
        >
          {message}
        </p>
      )}
      {canInstall && inSession && (
        <p className="update__message" data-testid="settings-update-in-session">
          {t("settings.update.inSession")}
        </p>
      )}
      {support !== null && support !== "none" && (
        <div className="update__actions">
          <button
            type="button"
            className="update__button"
            onClick={onCheck}
            disabled={working}
            data-testid="settings-update-check"
          >
            {t("settings.update.check")}
          </button>
          {canInstall && (
            <button
              type="button"
              className="update__button update__button--primary"
              onClick={onInstall}
              disabled={inSession || phase === "installing"}
              data-testid="settings-update-install"
            >
              {t("settings.update.install")}
            </button>
          )}
        </div>
      )}
    </section>
  );
}

export default UpdateSection;
