/**
 * PrivacyTab - What jamuru sends from this machine, and the questions about it
 *
 * Holds the usage reporting switch (ADR-037) and the one-off question about a
 * hang found at startup (ADR-056). Both are decisions about sending data, so
 * they sit apart from the diagnostics that help the user fix their setup.
 * Shares its styling with the diagnostics tab.
 */

import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import type { Hang, HangStage } from "../../../lib/tauri";
import "./TabContent.css";
import "./DiagnosticsTab.css";

export interface PrivacyTabProps {
  /** A hang found at startup while usage reporting is off; `null`/undefined = nothing to ask about */
  previousHang?: Hang | null;
  /** Answers the pending hang report (renders the section while set, together with `previousHang`) */
  onSendPreviousHang?: (send: boolean) => void;
  /** Set once the pending hang report has been answered, to show a short confirmation instead */
  previousHangAnswered?: boolean;
  /** Whether "send" also attaches the current jamuru.log (ADR-059: true before the app's 1.0.0 release) */
  previousHangAttachesLog?: boolean;
  /** Whether usage reporting is on (off by default) */
  usageReporting?: boolean;
  /** Turn usage reporting on or off (renders the usage reporting section) */
  onUsageReportingChange?: (enabled: boolean) => void;
  /** The lines the next report will contain; null/undefined = not shown yet */
  usagePreview?: string | null;
  /** Show (or refresh) what would be sent */
  onShowUsagePreview?: () => void;
  /** Why what would be sent could not be read */
  usagePreviewError?: string | null;
}

function stageLabel(stage: HangStage, t: TFunction): string {
  switch (stage) {
    case "app_exit":
      return t("settings.privacy.previousHang.stageAppExit", "closing");
    case "restart":
      return t("settings.privacy.previousHang.stageRestart", "restarting");
    case "update_apply":
      return t("settings.privacy.previousHang.stageUpdateApply", "installing an update");
    case "device_open":
      return t("settings.privacy.previousHang.stageDeviceOpen", "opening an audio device");
    case "unknown":
      return t("settings.privacy.previousHang.stageUnknown", "running");
  }
}

function PreviousHangSection({
  previousHang,
  onSendPreviousHang,
  previousHangAnswered,
  previousHangAttachesLog,
}: Pick<
  PrivacyTabProps,
  "previousHang" | "onSendPreviousHang" | "previousHangAnswered" | "previousHangAttachesLog"
>) {
  const { t } = useTranslation();

  if (!onSendPreviousHang || !previousHang) return null;

  if (previousHangAnswered) {
    return (
      <div className="diagnostics-tab__usage" data-testid="privacy-previous-hang">
        <p className="diagnostics-tab__description">
          {t("settings.privacy.previousHang.thanks", "Okay.")}
        </p>
      </div>
    );
  }

  return (
    <div className="diagnostics-tab__usage" data-testid="privacy-previous-hang">
      <span className="diagnostics-tab__problems-title">
        {t("settings.privacy.previousHang.title", "Last time")}
      </span>
      <p className="diagnostics-tab__description">
        {previousHangAttachesLog
          ? t(
              "settings.privacy.previousHang.descriptionWithLog",
              "jamuru did not close normally last time - it seems to have been stuck while {{stage}}. Send a small report, together with the current jamuru.log, so this can be found and fixed? The log can include the server address and your audio device names. The room ID and other participants' identifiers are cut down to their first characters, and their names are replaced by numbers.",
              { stage: stageLabel(previousHang.stage, t) }
            )
          : t(
              "settings.privacy.previousHang.description",
              "jamuru did not close normally last time - it seems to have been stuck while {{stage}}. Send a small report (nothing about your audio, chat or rooms) so this can be found and fixed?",
              { stage: stageLabel(previousHang.stage, t) }
            )}
      </p>
      <div className="diagnostics-tab__usage-actions">
        <button
          className="diagnostics-tab__rerun-btn"
          onClick={() => onSendPreviousHang(true)}
          type="button"
          data-testid="privacy-previous-hang-send"
        >
          {t("settings.privacy.previousHang.send", "Send report")}
        </button>
        <button
          className="diagnostics-tab__rerun-btn"
          onClick={() => onSendPreviousHang(false)}
          type="button"
          data-testid="privacy-previous-hang-dismiss"
        >
          {t("settings.privacy.previousHang.dismiss", "Don't send")}
        </button>
      </div>
    </div>
  );
}

function UsageReportingSection({
  usageReporting = false,
  onUsageReportingChange,
  usagePreview,
  onShowUsagePreview,
  usagePreviewError,
}: Pick<
  PrivacyTabProps,
  | "usageReporting"
  | "onUsageReportingChange"
  | "usagePreview"
  | "onShowUsagePreview"
  | "usagePreviewError"
>) {
  const { t } = useTranslation();

  if (!onUsageReportingChange) return null;

  const previewShown = usagePreview !== null && usagePreview !== undefined;

  return (
    <div className="diagnostics-tab__usage" data-testid="privacy-usage">
      <span className="diagnostics-tab__problems-title">
        {t("settings.privacy.usageTitle", "Usage data")}
      </span>
      <label className="diagnostics-tab__switch">
        <input
          type="checkbox"
          role="switch"
          checked={usageReporting}
          onChange={(e) => onUsageReportingChange(e.target.checked)}
          data-testid="privacy-usage-toggle"
          data-checked={usageReporting}
        />
        <span>{t("settings.privacy.usageToggle", "Send usage data")}</span>
      </label>
      <p className="diagnostics-tab__description">
        {t(
          "settings.privacy.usageDescription",
          "Off by default. When you turn it on, jamuru sends how it runs on your machine to the jamuru server, so that problems on particular machines and connections can be found and fixed."
        )}
      </p>
      <p className="diagnostics-tab__description">
        {t(
          "settings.privacy.usageSent",
          "Sent: app version, OS, CPU and memory, audio device names, IDs and what they support, your settings (a custom server URL only as far as its host and port), totals for each session (length, latency, packet loss, what kind of route the audio took and how long it took to connect), your own IP addresses (on your networks and the public one), the kind of each error, and where a crash happened."
        )}
      </p>
      <p className="diagnostics-tab__description">
        {t(
          "settings.privacy.usageNotSent",
          "Never sent: display name, room history, machine identifier, anything about the other people in the room (their IP addresses included), audio, chat."
        )}
      </p>
      <p className="diagnostics-tab__description">
        {t(
          "settings.privacy.usageDeviceNames",
          "Device names are sent as your system reports them. If a name contains your own name, such as \"Taro's AirPods\", it is sent too. Check the exact text with \"Show what is sent\"."
        )}
      </p>
      <p className="diagnostics-tab__description">
        {t(
          "settings.privacy.usageIdNote",
          "Turning it on creates a random install ID that ties your reports together. Turning it off discards the ID and anything not yet sent."
        )}
      </p>
      <div>
        <button
          className="diagnostics-tab__rerun-btn"
          onClick={onShowUsagePreview}
          type="button"
          data-testid="privacy-usage-show"
        >
          {t("settings.privacy.usageShow", "Show what is sent")}
        </button>
      </div>
      {previewShown && (
        <div data-testid="privacy-usage-preview">
          {usagePreview.trim() === "" ? (
            <p className="diagnostics-tab__description">
              {usageReporting
                ? t("settings.privacy.usagePreviewEmpty", "Nothing is waiting to be sent yet.")
                : t(
                    "settings.privacy.usagePreviewOff",
                    "Off: nothing is collected or sent, and there is no install ID."
                  )}
            </p>
          ) : (
            <pre className="diagnostics-tab__usage-lines">{usagePreview}</pre>
          )}
        </div>
      )}
      {usagePreviewError && (
        <p className="diagnostics-tab__log-error" role="alert">
          {usagePreviewError}
        </p>
      )}
    </div>
  );
}

export function PrivacyTab({
  previousHang,
  onSendPreviousHang,
  previousHangAnswered,
  previousHangAttachesLog,
  usageReporting,
  onUsageReportingChange,
  usagePreview,
  onShowUsagePreview,
  usagePreviewError,
}: PrivacyTabProps) {
  return (
    <div className="tab-content privacy-tab">
      <PreviousHangSection
        previousHang={previousHang}
        onSendPreviousHang={onSendPreviousHang}
        previousHangAnswered={previousHangAnswered}
        previousHangAttachesLog={previousHangAttachesLog}
      />
      <UsageReportingSection
        usageReporting={usageReporting}
        onUsageReportingChange={onUsageReportingChange}
        usagePreview={usagePreview}
        onShowUsagePreview={onShowUsagePreview}
        usagePreviewError={usagePreviewError}
      />
    </div>
  );
}

export default PrivacyTab;
