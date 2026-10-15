/**
 * HelpLogScreen - the window the person being helped keeps beside the app to
 * see what the help did (ADR-044 §5). It reads the helped app's record of the
 * help, adds each line as it happens, and says how long until the help ends by
 * itself. Stop is here too; the main window, which holds the connection, does
 * the stopping.
 */
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { SettingsHelpBar, SettingsHelpLog, settingLabel, type SettingsHelpLogLine } from "../components/SettingsHelp";
import { formatRemaining, useHelpRemaining } from "../components/SettingsHelp/useHelpRemaining";
import { useWindowEmit, useWindowEvent } from "../hooks/useWindowEvents";
import {
  HELP_LOG_EVENT,
  HELP_STOP_REQUESTED_EVENT,
  SESSION_SETTINGS_HELP,
  helpRecordDismiss,
  helpRecordGet,
  helpUndo,
  type HelpEntry,
  type HelpRecord,
} from "../lib/tauri";

/** The URL hash of the log window (`help_record.rs` opens it) */
export const HELP_LOG_HASH = "#/help-log";

/** A line is the same line however many times it is heard: an entry is told by when and what */
function entryId(entry: HelpEntry): string {
  return `${entry.at_ms}-${entry.kind}-${entry.kind === "setting_changed" ? entry.setting : ""}`;
}

export function HelpLogScreen() {
  const { t } = useTranslation();
  const emit = useWindowEmit();
  const [record, setRecord] = useState<HelpRecord | null>(null);
  const [undone, setUndone] = useState<string | null>(null);
  const { ms: remainingMs, seed } = useHelpRemaining(record?.active === true);

  const read = useCallback(() => {
    helpRecordGet()
      .then((next) => {
        setRecord(next);
        seed(next?.remaining_ms ?? null);
      })
      .catch((e) => console.error("Could not read what the help did:", e));
  }, [seed]);
  useEffect(read, [read]);

  // A line told here may also be in what was read: it is kept once.
  const onLogged = useCallback((entry: HelpEntry) => {
    setRecord((prev) => {
      if (prev === null || prev.entries.some((e) => entryId(e) === entryId(entry))) return prev;
      return { ...prev, entries: [...prev.entries, entry] };
    });
  }, []);
  useWindowEvent<HelpEntry>(HELP_LOG_EVENT, onLogged);
  // The help starting, ending or being asked for again changes what the record is.
  useWindowEvent<unknown>(SESSION_SETTINGS_HELP, read);

  const undo = useCallback(() => {
    helpUndo()
      .then((result) => {
        if (result.failed.length === 0) {
          setUndone(t("settingsHelp.report.restored"));
          helpRecordDismiss().catch((e) => console.error("Could not clear the record of the help:", e));
        } else {
          setUndone(
            t("settingsHelp.report.notRestored", {
              settings: result.failed
                .map((f) => settingLabel(f.setting, t))
                .join(t("settingsHelp.report.separator")),
            })
          );
        }
        read();
      })
      .catch((e) => console.error("Could not put the settings back:", e));
  }, [read, t]);

  const undoneLine =
    undone !== null ? (
      <p className="settings-help-log__remaining" role="status" data-testid="settings-help-log-undone">
        {undone}
      </p>
    ) : null;

  if (record === null) {
    return <SettingsHelpLog heading={t("settingsHelp.log.none")} lines={[]} emptyText="" after={undoneLine} />;
  }

  const name = record.helper_name;
  const lines: SettingsHelpLogLine[] = record.entries.map((entry) => ({
    id: entryId(entry),
    time: new Date(entry.at_ms).toLocaleTimeString(),
    text:
      entry.kind === "setting_changed"
        ? t("settingsHelp.log.settingChanged", { name, setting: settingLabel(entry.setting, t) })
        : t(`settingsHelp.log.${entry.kind}`, { name }),
  }));
  const pending = record.changed.length > 0 && !record.active ? record : null;
  const after = (
    <>
      {pending !== null && (
        <SettingsHelpBar
          message={t("settingsHelp.report.changed", {
            name,
            settings: pending.changed
              .map((setting) => settingLabel(setting, t))
              .join(t("settingsHelp.report.separator")),
          })}
          actions={
            pending.can_undo
              ? [{ label: t("settingsHelp.report.undo"), onClick: undo, testId: "settings-help-log-undo" }]
              : []
          }
        />
      )}
      {undoneLine}
    </>
  );

  return (
    <SettingsHelpLog
      heading={record.active ? t("settingsHelp.log.heading", { name }) : t("settingsHelp.log.headingOver", { name })}
      remaining={
        record.active && remainingMs !== null
          ? t("settingsHelp.helped.remaining", { time: formatRemaining(remainingMs) })
          : null
      }
      lines={lines}
      emptyText={t("settingsHelp.log.empty")}
      after={after}
      stopLabel={record.active ? t("settingsHelp.log.stop") : undefined}
      onStop={() => void emit(HELP_STOP_REQUESTED_EVENT)}
    />
  );
}

export default HelpLogScreen;
