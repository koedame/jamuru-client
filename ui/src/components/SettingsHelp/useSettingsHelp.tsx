/**
 * Helping with settings in the main window (ADR-044 §5): the state of the help
 * this app gives and receives, built from the events the backend sends, and what
 * to show for it - the asking, the question the helper answers, and the bar
 * that shows help is going on (with Stop).
 *
 * The person who wants help asks someone from the room; that person says yes
 * or not now. The help itself is not drawn here. The person helping works in a
 * window of their own that shows the helped app's screen (`HelperScreen`); the
 * person helped keeps working in this one, with the bar to stop it whenever
 * they like. The bar also says how long until the help ends by itself, and
 * opens the log of what the helper did. Muting and unmuting by the helper are
 * told here at once; when the help is over, what it changed is told with a way
 * to put it back.
 */
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useWindowEvent } from "../../hooks/useWindowEvents";
import {
  HELP_LOG_EVENT,
  HELP_STOP_REQUESTED_EVENT,
  PEER_MESSAGE_FEATURE,
  helpLogWindowOpen,
  helpRecordDismiss,
  helpRecordGet,
  helpUndo,
  settingsHelpAnswer,
  settingsHelpAsk,
  settingsHelpBegin,
  settingsHelpStop,
  type HelpEntry,
  type HelpEvent,
  type Participant,
} from "../../lib/tauri";
import { Toast } from "../Toast";
import { SettingsHelpAsk } from "./SettingsHelpAsk";
import { SettingsHelpBar } from "./SettingsHelpBar";
import { SettingsHelpQuestion } from "./SettingsHelpQuestion";
import { settingLabel } from "./settingText";
import { formatRemaining, useHelpRemaining } from "./useHelpRemaining";

/**
 * This app helping someone. `name` is kept from when the help began, so it
 * can still be said after they leave the participant list. `serial` numbers
 * each ask this app was shown, so a question for a new ask is a new question
 * even from the same person.
 */
type Giving =
  | { status: "asked"; peer: string; name: string; serial: number }
  /** Said yes; the help begins once the other app has opened its end of the connection */
  | { status: "starting"; peer: string; name: string }
  | { status: "active"; peer: string; name: string };

/** Someone helping this app, or asked to */
type Receiving = { status: "asking" | "starting" | "active"; peer: string; name: string };

/** What a help that is over changed, told to the person helped */
interface Report {
  name: string;
  /** The settings changed, by name */
  changed: string[];
  canUndo: boolean;
}

/** How long a passing notice (declined, ended) stays up */
const NOTICE_MS = 6000;

/** How long an ask waits for an answer before it is let go */
export const ASK_WAIT_MS = 120_000;

export interface SettingsHelp {
  /** Hand every SettingsHelp event the backend sends to this */
  onEvent: (event: HelpEvent) => void;
  /** Whether this app can ask for help now: it is not helped, and has not asked */
  canAsk: boolean;
  /** Start asking: shows what it means and who can be asked */
  openAsk: () => void;
  /** Who is helping this app now (their name), or null while no help goes on */
  helpedBy: string | null;
  /** The bars for help going on, for the session screen */
  bars: ReactNode;
  /** The asking and the question of whether to help, drawn over the screen */
  overlays: ReactNode;
}

export function useSettingsHelp(connId: number | null, participants: Participant[]): SettingsHelp {
  const { t } = useTranslation();
  const [giving, setGiving] = useState<Giving | null>(null);
  const [receiving, setReceiving] = useState<Receiving | null>(null);
  const [asking, setAsking] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [report, setReport] = useState<Report | null>(null);

  const participantsRef = useRef(participants);
  participantsRef.current = participants;
  const nameOf = useCallback(
    (peer: string) => participantsRef.current.find((p) => p.id === peer)?.name ?? "",
    []
  );
  // The help as last rendered, for the name of someone who has left the list.
  const givingRef = useRef(giving);
  givingRef.current = giving;
  const receivingRef = useRef(receiving);
  receivingRef.current = receiving;
  const asks = useRef(0);

  useEffect(() => {
    if (notice === null) return;
    const timer = setTimeout(() => setNotice(null), NOTICE_MS);
    return () => clearTimeout(timer);
  }, [notice]);

  const { ms: remainingMs, seed: seedRemaining } = useHelpRemaining(receiving?.status === "active");

  // What a help that is over changed, if it changed anything. Read once the
  // backend has ended the help, so the record is whole.
  const showReport = useCallback((name: string) => {
    helpRecordGet()
      .then((record) => {
        if (record === null) return;
        if (record.changed.length === 0) {
          helpRecordDismiss().catch((e) => console.error("Could not clear the record of the help:", e));
          return;
        }
        setReport({ name, changed: record.changed, canUndo: record.can_undo });
      })
      .catch((e) => console.error("Could not read what the help did:", e));
  }, []);

  const stop = useCallback(
    (role: "helper" | "helped") => {
      if (connId === null) return;
      const reportName = role === "helped" && receivingRef.current?.status === "active" ? receivingRef.current.name : null;
      if (role === "helper") setGiving(null);
      else setReceiving(null);
      settingsHelpStop(connId, role)
        .then(() => {
          if (reportName !== null) showReport(reportName);
        })
        .catch((e) => console.error("Could not stop the help with settings:", e));
    },
    [connId, showReport]
  );

  // Stop pressed in the log window, which has no hold of the connection.
  const onStopRequested = useCallback(() => {
    if (receivingRef.current?.status === "active") stop("helped");
  }, [stop]);
  useWindowEvent<void>(HELP_STOP_REQUESTED_EVENT, onStopRequested);

  // The helper unmuting the microphone is the one thing the person would
  // otherwise not see happen: it is told here, and the log has the rest.
  const onLogged = useCallback(
    (entry: HelpEntry) => {
      const current = receivingRef.current;
      if (current?.status !== "active") return;
      if (entry.kind === "muted") setNotice(t("settingsHelp.log.muted", { name: current.name }));
      if (entry.kind === "unmuted") setNotice(t("settingsHelp.log.unmuted", { name: current.name }));
    },
    [t]
  );
  useWindowEvent<HelpEntry>(HELP_LOG_EVENT, onLogged);

  const undo = useCallback(() => {
    helpUndo()
      .then((result) => {
        const labels = (settings: string[]) =>
          settings.map((setting) => settingLabel(setting, t)).join(t("settingsHelp.report.separator"));
        if (result.failed.length === 0) {
          setReport(null);
          setNotice(t("settingsHelp.report.restored"));
          helpRecordDismiss().catch((e) => console.error("Could not clear the record of the help:", e));
          return;
        }
        const failed = result.failed.map((f) => f.setting);
        setReport((prev) => (prev ? { ...prev, changed: failed, canUndo: true } : prev));
        setNotice(t("settingsHelp.report.notRestored", { settings: labels(failed) }));
      })
      .catch((e) => console.error("Could not put the settings back:", e));
  }, [t]);

  const dismissReport = useCallback(() => {
    setReport(null);
    helpRecordDismiss().catch((e) => console.error("Could not clear the record of the help:", e));
  }, []);

  // An ask nobody answers is let go, so the person is not left waiting.
  const waitingFor = receiving?.status === "asking" ? receiving : null;
  useEffect(() => {
    if (waitingFor === null) return;
    const timer = setTimeout(() => {
      stop("helped");
      setNotice(t("settingsHelp.ask.noAnswer", { name: waitingFor.name }));
    }, ASK_WAIT_MS);
    return () => clearTimeout(timer);
  }, [waitingFor, stop, t]);

  const begin = useCallback(
    (peer: string, name: string) => {
      if (connId === null) return;
      settingsHelpBegin(connId, peer).catch((e) => {
        console.error("Could not start the help with settings:", e);
        // The help did not start, so nothing else will end the wait.
        setReceiving((prev) => (prev?.status === "starting" && prev.peer === peer ? null : prev));
        setNotice(t("settingsHelp.ask.couldNotStart", { name }));
      });
    },
    [connId, t]
  );

  const onEvent = useCallback(
    (event: HelpEvent) => {
      switch (event.type) {
        case "asked":
          asks.current += 1;
          // The next event of the same poll reads the ref before this renders.
          givingRef.current = { status: "asked", peer: event.peer, name: event.peer_name, serial: asks.current };
          setGiving(givingRef.current);
          break;
        case "taken":
          setReceiving({ status: "starting", peer: event.peer, name: event.peer_name });
          begin(event.peer, event.peer_name);
          break;
        case "started":
          if (event.role === "helper") {
            setGiving({ status: "active", peer: event.peer, name: event.peer_name });
          } else {
            setReceiving({ status: "active", peer: event.peer, name: event.peer_name });
            setReport(null);
            helpRecordGet()
              .then((record) => seedRemaining(record?.remaining_ms ?? null))
              .catch((e) => console.error("Could not read the time left of the help:", e));
          }
          break;
        case "declined": {
          const name = (event.role === "helped" ? receivingRef.current : givingRef.current)?.name || nameOf(event.peer);
          if (event.role === "helped") {
            setReceiving(null);
            setNotice(t(event.busy ? "settingsHelp.ask.busy" : "settingsHelp.ask.declined", { name }));
          } else {
            setGiving(null);
            setNotice(t("settingsHelp.ask.couldNotStart", { name }));
          }
          break;
        }
        case "ended": {
          // Every end describes the app's help as it is now (stopping here
          // clears the help without one), so it ends the help with that peer.
          const current = event.role === "helper" ? givingRef.current : receivingRef.current;
          if (event.role === "helper") setGiving((prev) => (prev?.peer === event.peer ? null : prev));
          else setReceiving((prev) => (prev?.peer === event.peer ? null : prev));
          const name = (current?.peer === event.peer ? current.name : "") || nameOf(event.peer);
          if (event.role === "helped" && current?.status === "active" && current.peer === event.peer) showReport(name);
          if (event.reason === "timed_out") setNotice(t("settingsHelp.ended.timedOut", { name }));
          if (event.reason === "peer_stopped") {
            const withdrawn = event.role === "helper" && current?.status === "asked";
            setNotice(t(withdrawn ? "settingsHelp.ask.withdrawn" : "settingsHelp.ended.stopped", { name }));
          }
          if (event.reason === "peer_left") setNotice(t("settingsHelp.ended.left", { name }));
          break;
        }
      }
    },
    [begin, nameOf, seedRemaining, showReport, t]
  );

  const candidates = participants.filter((p) => (p.features ?? []).includes(PEER_MESSAGE_FEATURE));
  const canAsk = receiving === null && connId !== null;

  const ask = useCallback(
    (peerId: string) => {
      if (connId === null) return;
      const name = nameOf(peerId);
      setAsking(false);
      settingsHelpAsk(connId, peerId)
        .then(() => setReceiving({ status: "asking", peer: peerId, name }))
        .catch((e) => console.error("Could not ask for help with settings:", e));
    },
    [connId, nameOf]
  );

  const answer = useCallback(
    (peer: string, accept: boolean) => {
      if (connId === null) return;
      // Not now starts nothing, so nothing will come back to clear the question.
      // Yes takes the other app a moment (it opens its end of the connection), so
      // the question closes now and the help is shown when it has started.
      setGiving((prev) =>
        accept && prev?.status === "asked" ? { status: "starting", peer: prev.peer, name: prev.name } : null
      );
      settingsHelpAnswer(connId, peer, accept).catch((e) => {
        console.error("Could not answer the ask for help:", e);
        // The help did not start, so nothing else will end the wait.
        setGiving((prev) => (prev?.status === "starting" && prev.peer === peer ? null : prev));
      });
    },
    [connId]
  );

  const bars: ReactNode[] = [];
  if (receiving?.status === "asking") {
    bars.push(
      <SettingsHelpBar
        key="asking"
        message={t("settingsHelp.ask.waiting", { name: receiving.name })}
        actions={[{ label: t("settingsHelp.ask.cancel"), onClick: () => stop("helped"), testId: "settings-help-cancel" }]}
      />
    );
  }
  if (receiving?.status === "active") {
    bars.push(
      <SettingsHelpBar
        key="helped"
        message={t("settingsHelp.helped.bar", { name: receiving.name })}
        status={remainingMs === null ? null : t("settingsHelp.helped.remaining", { time: formatRemaining(remainingMs) })}
        actions={[
          {
            label: t("settingsHelp.helped.log"),
            onClick: () => helpLogWindowOpen().catch((e) => console.error("Could not open the log of the help:", e)),
            testId: "settings-help-log-open",
          },
          { label: t("settingsHelp.helped.stop"), onClick: () => stop("helped"), testId: "settings-help-stop-helped" },
        ]}
      />
    );
  }
  if (report !== null) {
    bars.push(
      <SettingsHelpBar
        key="report"
        message={t("settingsHelp.report.changed", {
          name: report.name,
          settings: report.changed.map((setting) => settingLabel(setting, t)).join(t("settingsHelp.report.separator")),
        })}
        actions={[
          ...(report.canUndo ? [{ label: t("settingsHelp.report.undo"), onClick: undo, testId: "settings-help-undo" }] : []),
          { label: t("settingsHelp.report.dismiss"), onClick: dismissReport, testId: "settings-help-report-dismiss" },
        ]}
      />
    );
  }
  if (giving?.status === "active") {
    bars.push(
      <SettingsHelpBar
        key="giving"
        message={t("settingsHelp.helper.bar", { name: giving.name })}
        actions={[{ label: t("settingsHelp.helper.stop"), onClick: () => stop("helper"), testId: "settings-help-stop-helper" }]}
      />
    );
  }
  if (notice) {
    bars.push(<Toast key="notice" type="info" message={notice} />);
  }

  // Each question is mounted afresh (and keyed), so Allow is held back from
  // its first frame and focus returns when it goes.
  const overlays = (
    <>
      {asking && (
        <SettingsHelpAsk
          title={t("settingsHelp.ask.title")}
          lines={[t("settingsHelp.ask.what"), t("settingsHelp.ask.limits"), t("settingsHelp.ask.control")]}
          chooseTitle={t("settingsHelp.ask.chooseTitle")}
          noneText={t("settingsHelp.ask.none")}
          candidates={candidates.map((p) => ({ id: p.id, name: p.name }))}
          chooseLabel={t("settingsHelp.ask.choose")}
          askLabel={t("settingsHelp.ask.send")}
          cancelLabel={t("settingsHelp.ask.never")}
          onAsk={ask}
          onCancel={() => setAsking(false)}
        />
      )}
      {giving?.status === "asked" && (
        <SettingsHelpQuestion
          key={`ask-${giving.serial}`}
          open
          message={t("settingsHelp.request.message", { name: giving.name })}
          detail={t("settingsHelp.request.detail", { name: giving.name })}
          allowLabel={t("settingsHelp.request.allow")}
          declineLabel={t("settingsHelp.request.decline")}
          onAllow={() => answer(giving.peer, true)}
          onDecline={() => answer(giving.peer, false)}
        />
      )}
    </>
  );

  return {
    onEvent,
    canAsk,
    openAsk: () => setAsking(true),
    helpedBy: receiving?.status === "active" ? receiving.name : null,
    bars: bars.length > 0 ? <div className="settings-help-bars">{bars}</div> : null,
    overlays,
  };
}
