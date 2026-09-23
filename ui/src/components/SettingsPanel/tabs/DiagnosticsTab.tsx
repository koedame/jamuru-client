/**
 * DiagnosticsTab - System diagnostics with results view
 *
 * Design: jamjam brand (ui.pen Screens/Settings/Diagnostics + Running + Result).
 * Idle: description + primary run button. Running: loader, progress bar, steps,
 * optional cancel. Complete: score, result cards with grade badges, problems.
 */

import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  CompleteDiagnosticsResult,
  DeviceDiagnostics,
  DeviceSource,
  DiagnosticGrade,
  DiagnosticProblem,
  ProblemCode,
  RecommendedPreset,
} from "../../../lib/tauri";
import {
  PlayIcon,
  RefreshIcon,
  CheckIcon,
  LoaderIcon,
  CircleIcon,
  WifiIcon,
  HeadphonesIcon,
  CpuIcon,
} from "../icons";
import "./TabContent.css";
import "./DiagnosticsTab.css";

export type DiagnosticsState = "idle" | "running" | "complete";

export interface DiagnosticsTabProps {
  /** Current diagnostics state */
  state?: DiagnosticsState;
  /** Progress percentage (0-100) during running state */
  progress?: number;
  /** Progress message during running state */
  progressMessage?: string;
  /** Diagnostics result (when state is complete) */
  result?: CompleteDiagnosticsResult;
  /** Run diagnostics handler */
  onRunDiagnostics?: () => void;
  /** Cancel diagnostics handler (renders a cancel button while running) */
  onCancelDiagnostics?: () => void;
  /** Apply recommended preset handler */
  onApplyPreset?: (preset: RecommendedPreset) => void;
  /** Open the folder holding the log file (renders the log file section) */
  onOpenLogFolder?: () => void;
  /** Folder that was opened, shown under the button */
  logFolder?: string | null;
  /** Why the folder could not be opened */
  logFolderError?: string | null;
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

type StepStatus = "done" | "active" | "pending";
type Tone = "default" | "good" | "warn" | "bad";

function gradeTone(grade?: DiagnosticGrade): Tone {
  switch (grade) {
    case "A":
      return "good";
    case "B":
      return "warn";
    case "C":
      return "bad";
    default:
      return "default";
  }
}

function severityTone(severity: string): Tone {
  switch (severity) {
    case "Error":
      return "bad";
    case "Warning":
      return "warn";
    default:
      return "default";
  }
}

function getPresetDisplayName(preset: RecommendedPreset, t: TFunction): string {
  switch (preset) {
    case "ZeroLatency":
      return t("diagnostics.presets.zeroLatency");
    case "UltraLowLatency":
      return t("diagnostics.presets.ultraLowLatency");
    case "Balanced":
      return t("diagnostics.presets.balanced");
    case "HighQuality":
      return t("diagnostics.presets.highQuality");
  }
}

function categoryLabel(category: string, t: TFunction): string {
  switch (category) {
    case "network":
      return t("settings.diagnostics.network", "Network");
    case "audio":
      return t("settings.diagnostics.audio", "Audio");
    case "cpu":
      return t("settings.diagnostics.cpu", "CPU");
    default:
      return category;
  }
}

/** Device name with a note on whether it's the configured device or the OS default. */
function deviceValueWithSource(
  device: DeviceDiagnostics | null | undefined,
  source: DeviceSource,
  t: TFunction
): string {
  if (!device) return t("settings.diagnostics.no", "None");
  const sourceLabel =
    source === "Configured"
      ? t("settings.diagnostics.deviceSourceConfigured", "your setting")
      : t("settings.diagnostics.deviceSourceOsDefault", "OS default");
  return `${device.name} (${sourceLabel})`;
}

/** locale key segment for a problem code, e.g. `HighJitter` -> `highJitter` */
function problemCodeKey(type: ProblemCode["type"]): string {
  return type.charAt(0).toLowerCase() + type.slice(1);
}

/** Interpolation values for a problem code's message/suggestion templates. */
function problemArgs(
  code: ProblemCode,
  t: TFunction
): Record<string, string | number> | undefined {
  switch (code.type) {
    case "HighJitter":
      return { jitter: code.data.jitter_ms.toFixed(1) };
    case "SignalingUnreachable":
      return {
        url: code.data.url,
        error: code.data.error ?? t("settings.diagnostics.unknownError"),
      };
    case "InputEnumerationFailed":
    case "OutputEnumerationFailed":
      return { error: code.data.error };
    case "InputNot48kHz":
    case "OutputNot48kHz":
      return { device: code.data.device_name };
    case "HighCpuUsage":
      return { usage: Math.round(code.data.usage_percent) };
    case "LowMemory":
      return { mb: code.data.available_mb };
    default:
      return undefined;
  }
}

function formatProblem(code: ProblemCode, t: TFunction): { message: string; suggestion: string } {
  const key = problemCodeKey(code.type);
  const args = problemArgs(code, t);
  return {
    message: t(`settings.diagnostics.problemCodes.${key}.message`, args),
    suggestion: t(`settings.diagnostics.problemCodes.${key}.suggestion`, args),
  };
}

function formatNatType(natType: string, t: TFunction): string {
  switch (natType) {
    case "NoNat":
      return t("diagnostics.natTypes.noNat");
    case "FullCone":
      return t("diagnostics.natTypes.fullCone");
    case "RestrictedCone":
      return t("diagnostics.natTypes.restrictedCone");
    case "PortRestrictedCone":
      return t("diagnostics.natTypes.portRestrictedCone");
    case "Symmetric":
      return t("diagnostics.natTypes.symmetric");
    default:
      return t("diagnostics.natTypes.unknown");
  }
}

/** Step status derived from overall progress (network → audio → cpu). */
function stepStatus(index: number, progress: number): StepStatus {
  const start = index * (100 / 3);
  const end = (index + 1) * (100 / 3);
  if (progress >= end) return "done";
  if (progress >= start) return "active";
  return "pending";
}

function StepIcon({ status }: { status: StepStatus }) {
  if (status === "done") return <CheckIcon />;
  if (status === "active") return <LoaderIcon className="diagnostics-tab__spin" />;
  return <CircleIcon />;
}

function GradeBadge({ grade }: { grade?: DiagnosticGrade }) {
  if (!grade || grade === "Unknown") return null;
  return (
    <span className={`diagnostics-tab__grade diagnostics-tab__grade--${gradeTone(grade)}`}>
      {grade}
    </span>
  );
}

function Row({ label, value, tone = "default" }: { label: string; value: string; tone?: Tone }) {
  return (
    <div className="diagnostics-tab__row">
      <span className="diagnostics-tab__row-label">{label}</span>
      <span className={`diagnostics-tab__row-value diagnostics-tab__row-value--${tone}`}>
        {value}
      </span>
    </div>
  );
}

function Card({
  icon,
  title,
  grade,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  grade?: DiagnosticGrade;
  children: React.ReactNode;
}) {
  return (
    <div className="diagnostics-tab__card">
      <div className="diagnostics-tab__card-header">
        <span className="diagnostics-tab__card-title">
          <span className="diagnostics-tab__card-icon" aria-hidden="true">
            {icon}
          </span>
          {title}
        </span>
        <GradeBadge grade={grade} />
      </div>
      <div className="diagnostics-tab__card-body">{children}</div>
    </div>
  );
}

function LogFileSection({
  onOpenLogFolder,
  logFolder,
  logFolderError,
}: Pick<DiagnosticsTabProps, "onOpenLogFolder" | "logFolder" | "logFolderError">) {
  const { t } = useTranslation();

  if (!onOpenLogFolder) return null;

  return (
    <div className="diagnostics-tab__log-file" data-testid="diagnostics-log-file">
      <span className="diagnostics-tab__problems-title">
        {t("settings.diagnostics.logTitle", "Log file")}
      </span>
      <p className="diagnostics-tab__description">
        {t(
          "settings.diagnostics.logDescription",
          "If something does not work, attach jamjam.log from this folder when you report it."
        )}
      </p>
      <div>
        <button
          className="diagnostics-tab__rerun-btn"
          onClick={onOpenLogFolder}
          type="button"
          data-testid="diagnostics-open-log-folder"
        >
          {t("settings.diagnostics.openLogFolder", "Open log folder")}
        </button>
      </div>
      {logFolder && <p className="diagnostics-tab__log-path">{logFolder}</p>}
      {logFolderError && (
        <p className="diagnostics-tab__log-error" role="alert">
          {logFolderError}
        </p>
      )}
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
  DiagnosticsTabProps,
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
    <div className="diagnostics-tab__usage" data-testid="diagnostics-usage">
      <span className="diagnostics-tab__problems-title">
        {t("settings.diagnostics.usageTitle", "Usage data")}
      </span>
      <label className="diagnostics-tab__switch">
        <input
          type="checkbox"
          role="switch"
          checked={usageReporting}
          onChange={(e) => onUsageReportingChange(e.target.checked)}
          data-testid="diagnostics-usage-toggle"
          data-checked={usageReporting}
        />
        <span>{t("settings.diagnostics.usageToggle", "Send usage data")}</span>
      </label>
      <p className="diagnostics-tab__description">
        {t(
          "settings.diagnostics.usageDescription",
          "Off by default. When you turn it on, jamjam sends how it runs on your machine to the jamjam server, so that problems on particular machines and connections can be found and fixed."
        )}
      </p>
      <p className="diagnostics-tab__description">
        {t(
          "settings.diagnostics.usageSent",
          "Sent: app version, OS, CPU and memory, audio device names and what they support, your settings, totals for each session (length, latency, packet loss), the kind of each error, and where a crash happened."
        )}
      </p>
      <p className="diagnostics-tab__description">
        {t(
          "settings.diagnostics.usageNotSent",
          "Never sent: display name, room history, custom server URL, device IDs, machine identifier, audio, chat."
        )}
      </p>
      <p className="diagnostics-tab__description">
        {t(
          "settings.diagnostics.usageDeviceNames",
          "Device names are sent as your system reports them. If a name contains your own name, such as \"Taro's AirPods\", it is sent too. Check the exact text with \"Show what is sent\"."
        )}
      </p>
      <p className="diagnostics-tab__description">
        {t(
          "settings.diagnostics.usageIdNote",
          "Turning it on creates a random install ID that ties your reports together. Turning it off discards the ID and anything not yet sent."
        )}
      </p>
      <div>
        <button
          className="diagnostics-tab__rerun-btn"
          onClick={onShowUsagePreview}
          type="button"
          data-testid="diagnostics-usage-show"
        >
          {t("settings.diagnostics.usageShow", "Show what is sent")}
        </button>
      </div>
      {previewShown && (
        <div data-testid="diagnostics-usage-preview">
          {usagePreview.trim() === "" ? (
            <p className="diagnostics-tab__description">
              {usageReporting
                ? t("settings.diagnostics.usagePreviewEmpty", "Nothing is waiting to be sent yet.")
                : t(
                    "settings.diagnostics.usagePreviewOff",
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

export function DiagnosticsTab({
  state = "idle",
  progress = 0,
  progressMessage = "",
  result,
  onRunDiagnostics,
  onCancelDiagnostics,
  onApplyPreset,
  onOpenLogFolder,
  logFolder,
  logFolderError,
  usageReporting,
  onUsageReportingChange,
  usagePreview,
  onShowUsagePreview,
  usagePreviewError,
}: DiagnosticsTabProps) {
  const { t } = useTranslation();
  const logFile = (
    <>
      <UsageReportingSection
        usageReporting={usageReporting}
        onUsageReportingChange={onUsageReportingChange}
        usagePreview={usagePreview}
        onShowUsagePreview={onShowUsagePreview}
        usagePreviewError={usagePreviewError}
      />
      <LogFileSection
        onOpenLogFolder={onOpenLogFolder}
        logFolder={logFolder}
        logFolderError={logFolderError}
      />
    </>
  );

  // Idle state
  if (state === "idle") {
    return (
      <div className="tab-content">
        <h2 className="tab-content__title">{t("settings.diagnostics.title", "System Diagnostics")}</h2>
        <p className="diagnostics-tab__description">
          {t(
            "settings.diagnostics.description",
            "Diagnose network, audio, and CPU performance to recommend optimal settings."
          )}
        </p>
        <button
          className="tab-content__primary-button"
          onClick={onRunDiagnostics}
          type="button"
        >
          <PlayIcon />
          {t("settings.diagnostics.run", "Run Diagnostics")}
        </button>
        {logFile}
      </div>
    );
  }

  // Running state
  if (state === "running") {
    const steps = [
      t("settings.diagnostics.stepNetwork", "Checking network connection"),
      t("settings.diagnostics.stepAudio", "Checking audio devices"),
      t("settings.diagnostics.stepCpu", "Testing CPU performance"),
    ];

    return (
      <div className="tab-content diagnostics-tab__running">
        <div className="diagnostics-tab__loader" role="status" aria-live="polite">
          <LoaderIcon size={48} className="diagnostics-tab__spin" />
        </div>
        <div className="diagnostics-tab__running-text">
          <h2 className="tab-content__title">{t("settings.diagnostics.running", "Running diagnostics...")}</h2>
          <p className="diagnostics-tab__description">
            {t("settings.diagnostics.runningDesc", "Checking system status")}
          </p>
        </div>

        <div className="diagnostics-tab__progress">
          <div className="diagnostics-tab__progress-bar">
            <div
              className="diagnostics-tab__progress-fill"
              style={{ width: `${progress}%` }}
            />
          </div>
          <ul className="diagnostics-tab__steps">
            {steps.map((label, i) => {
              const status = stepStatus(i, progress);
              return (
                <li key={i} className={`diagnostics-tab__step diagnostics-tab__step--${status}`}>
                  <span className="diagnostics-tab__step-icon" aria-hidden="true">
                    <StepIcon status={status} />
                  </span>
                  <span className="diagnostics-tab__step-label">
                    {status === "active" && progressMessage ? progressMessage : label}
                  </span>
                </li>
              );
            })}
          </ul>
        </div>

        {onCancelDiagnostics && (
          <button
            className="diagnostics-tab__cancel-btn"
            onClick={onCancelDiagnostics}
            type="button"
          >
            {t("settings.diagnostics.cancel", "Cancel")}
          </button>
        )}
      </div>
    );
  }

  // Complete state
  if (state === "complete" && result) {
    const { network, audio, cpu, problems } = result;

    return (
      <div className="tab-content">
        <div className="diagnostics-tab__score-row">
          <div className="diagnostics-tab__score">
            <span className="diagnostics-tab__score-value">{result.overall_score}</span>
            <span className="diagnostics-tab__score-max">/100</span>
          </div>
          <button
            className="diagnostics-tab__rerun-btn"
            onClick={onRunDiagnostics}
            type="button"
          >
            <RefreshIcon />
            {t("settings.diagnostics.rerun", "Run Again")}
          </button>
        </div>

        <Card
          icon={<WifiIcon />}
          title={t("settings.diagnostics.network", "Network")}
          grade={network.connection_stability}
        >
          <Row
            label={t("settings.diagnostics.publicIp", "Public IP")}
            value={network.ip_support.public_ipv4 || t("settings.diagnostics.notAvailable")}
          />
          <Row
            label={t("settings.diagnostics.natType", "NAT Type")}
            value={formatNatType(network.nat_type, t)}
          />
          <Row
            label={t("settings.diagnostics.rtt", "RTT")}
            value={
              network.stability_metrics.avg_rtt_ms !== null
                ? `${Math.round(network.stability_metrics.avg_rtt_ms)} ms`
                : t("settings.diagnostics.notAvailable")
            }
            tone={
              network.stability_metrics.avg_rtt_ms !== null &&
              network.stability_metrics.avg_rtt_ms <= 30
                ? "good"
                : "default"
            }
          />
          <Row
            label={t("settings.diagnostics.jitter", "Jitter")}
            value={network.jitter_ms !== null ? `${Math.round(network.jitter_ms)} ms` : t("settings.diagnostics.notAvailable")}
            tone={network.jitter_ms !== null && network.jitter_ms <= 10 ? "good" : "default"}
          />
          <Row
            label={t("settings.diagnostics.packetLoss", "Packet Loss")}
            value={`${(network.stability_metrics.packet_loss_rate * 100).toFixed(1)}%`}
            tone={network.stability_metrics.packet_loss_rate < 0.01 ? "good" : "default"}
          />
        </Card>

        <Card
          icon={<HeadphonesIcon />}
          title={t("settings.diagnostics.audio", "Audio")}
          grade={audio.overall_grade}
        >
          <Row
            label={t("settings.diagnostics.inputDevice", "Input")}
            value={deviceValueWithSource(audio.selected_input, audio.input_source, t)}
          />
          <Row
            label={t("settings.diagnostics.outputDevice", "Output")}
            value={deviceValueWithSource(audio.selected_output, audio.output_source, t)}
          />
          <Row
            label={t("settings.diagnostics.supports48khz", "48 kHz")}
            value={audio.selected_input?.supports_48khz ? t("settings.diagnostics.yes", "Yes") : t("settings.diagnostics.no", "No")}
          />
          <Row
            label={t("settings.diagnostics.minBuffer", "Min Buffer")}
            value={
              audio.low_latency_support.min_buffer_size !== null
                ? t("settings.diagnostics.samples", { count: audio.low_latency_support.min_buffer_size })
                : t("settings.diagnostics.notAvailable")
            }
          />
          <Row
            label={t("settings.diagnostics.estimatedLatency", "Est. Latency")}
            value={
              audio.low_latency_support.estimated_min_latency_ms !== null
                ? `${audio.low_latency_support.estimated_min_latency_ms.toFixed(1)} ms`
                : t("settings.diagnostics.notAvailable")
            }
          />
        </Card>

        <Card icon={<CpuIcon />} title={t("settings.diagnostics.cpu", "CPU")} grade={cpu.grade}>
          <Row
            label={t("settings.diagnostics.cores", "Cores")}
            value={String(cpu.system.cpu_cores)}
          />
          <Row
            label={t("settings.diagnostics.realtime", "Realtime")}
            value={cpu.realtime_capable ? t("settings.diagnostics.yes", "Yes") : t("settings.diagnostics.no", "No")}
            tone={cpu.realtime_capable ? "good" : "warn"}
          />
          {(() => {
            const bench = cpu.benchmarks.find((b) => b.buffer_size === 64);
            return bench ? (
              <Row
                label={t("settings.diagnostics.processingTime", "Processing")}
                value={`${bench.processing_time_us} µs`}
              />
            ) : null;
          })()}
        </Card>

        <Card icon={<PlayIcon />} title={t("settings.diagnostics.recommendation", "Recommendation")}>
          <Row
            label={t("settings.diagnostics.recommendedPreset", "Preset")}
            value={getPresetDisplayName(result.recommended_preset, t)}
          />
          <Row
            label={t("settings.diagnostics.zeroLatency", "Zero Latency")}
            value={
              result.zero_latency_compatible
                ? t("settings.diagnostics.compatible", "Compatible")
                : t("settings.diagnostics.notCompatible", "Not Compatible")
            }
            tone={result.zero_latency_compatible ? "good" : "warn"}
          />
          {onApplyPreset && (
            <button
              className="diagnostics-tab__apply-btn"
              onClick={() => onApplyPreset(result.recommended_preset)}
              type="button"
            >
              {t("settings.diagnostics.applyPreset", "Apply Preset")}
            </button>
          )}
        </Card>

        <ProblemsSection problems={problems} />
        {logFile}
      </div>
    );
  }

  return null;
}

function ProblemsSection({ problems }: { problems: DiagnosticProblem[] }) {
  const { t } = useTranslation();

  const severityLabel = (severity: string): string => {
    switch (severity) {
      case "Error":
        return t("settings.diagnostics.severity.error", "Error");
      case "Warning":
        return t("settings.diagnostics.severity.warning", "Warning");
      default:
        return t("settings.diagnostics.severity.info", "Info");
    }
  };

  return (
    <div className="diagnostics-tab__problems">
      <div className="diagnostics-tab__problems-header">
        <span className="diagnostics-tab__problems-title">
          {t("settings.diagnostics.problems", "Problems")}
        </span>
        {problems.length > 0 && (
          <span className="diagnostics-tab__problems-count">{problems.length}</span>
        )}
      </div>

      {problems.length === 0 ? (
        <p className="diagnostics-tab__no-problems">
          {t("settings.diagnostics.noProblems", "No problems detected")}
        </p>
      ) : (
        <div className="diagnostics-tab__problems-list">
          {problems.map((problem, index) => {
            const { message, suggestion } = formatProblem(problem.code, t);
            return (
              <div key={index} className="diagnostics-tab__problem-card">
                <div className="diagnostics-tab__problem-badges">
                  <span className="diagnostics-tab__problem-category">
                    {categoryLabel(problem.category, t)}
                  </span>
                  <span
                    className={`diagnostics-tab__problem-severity diagnostics-tab__problem-severity--${severityTone(problem.severity)}`}
                  >
                    {severityLabel(problem.severity)}
                  </span>
                </div>
                <p className="diagnostics-tab__problem-message">{message}</p>
                {suggestion && (
                  <p className="diagnostics-tab__problem-suggestion">{suggestion}</p>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

export default DiagnosticsTab;
