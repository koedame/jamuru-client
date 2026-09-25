/**
 * Helping with settings in the main window (ADR-043): the state of the help
 * this app gives and receives, built from the events the room's polling
 * delivers, and what to show for it - the questions the helped side answers,
 * the bar that shows help is going on (with Stop), and the helper's panel.
 */
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  PEER_MESSAGE_FEATURE,
  settingsGet,
  settingsHelpAnswer,
  settingsHelpDecide,
  settingsHelpPropose,
  settingsHelpRequest,
  settingsHelpStop,
  type AudioSettings,
  type HelpEvent,
  type PeerInfo,
  type SettingChange,
} from "../../lib/tauri";
import { SidePanel } from "../SidePanel";
import { Toast } from "../Toast";
import { useAudioSettingsTab } from "../SettingsPanel/useAudioSettingsTab";
import { SettingsHelpBar } from "./SettingsHelpBar";
import { SettingsHelpPanel } from "./SettingsHelpPanel";
import { SettingsHelpQuestion } from "./SettingsHelpQuestion";
import { changeValue, settingLabel } from "./settingText";

/** This app helping someone */
type Giving =
  | { status: "asking"; peer: string }
  | {
      status: "active";
      peer: string;
      settings: AudioSettings | null;
      /** The proposal not answered yet: one at a time */
      waiting: number | null;
      /** The last answer worth saying (a decline or a refusal) */
      note: string | null;
      panelOpen: boolean;
    };

/** Someone helping this app */
type Receiving =
  | { status: "asked"; peer: string; name: string }
  | {
      status: "active";
      peer: string;
      name: string;
      /** The change the user is asked about (the app asks one at a time); its id is the app's number for it */
      question: { id: number; change: SettingChange } | null;
    };

/** How long a passing notice (declined, ended) stays up */
const NOTICE_MS = 6000;

export interface SettingsHelp {
  /** Hand every SettingsHelp event of the room's polling to this */
  onEvent: (event: HelpEvent) => void;
  /** Whether this app can offer `peer` help now */
  canOffer: (peer: PeerInfo) => boolean;
  offer: (peerId: string) => void;
  /** The bars for help going on, for the session screen */
  bars: ReactNode;
  /** Questions and the helper's panel, drawn over the screen */
  overlays: ReactNode;
}

export function useSettingsHelp(connId: number | null, participants: PeerInfo[]): SettingsHelp {
  const { t } = useTranslation();
  const [giving, setGiving] = useState<Giving | null>(null);
  const [receiving, setReceiving] = useState<Receiving | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // This app's own settings, to name the device a proposal would switch to.
  const [ownSettings, setOwnSettings] = useState<AudioSettings | null>(null);

  const participantsRef = useRef(participants);
  participantsRef.current = participants;
  const nameOf = useCallback(
    (peer: string) => participantsRef.current.find((p) => p.id === peer)?.name ?? "",
    []
  );

  useEffect(() => {
    if (notice === null) return;
    const timer = setTimeout(() => setNotice(null), NOTICE_MS);
    return () => clearTimeout(timer);
  }, [notice]);

  const onEvent = useCallback(
    (event: HelpEvent) => {
      switch (event.type) {
        case "requested":
          setReceiving({ status: "asked", peer: event.peer, name: event.peer_name });
          break;
        case "started":
          if (event.role === "helper") {
            setGiving({
              status: "active",
              peer: event.peer,
              settings: event.settings,
              waiting: null,
              note: null,
              panelOpen: true,
            });
          } else {
            setReceiving((prev) => ({
              status: "active",
              peer: event.peer,
              name: prev?.peer === event.peer ? prev.name : nameOf(event.peer),
              question: null,
            }));
          }
          break;
        case "declined":
          setGiving(null);
          setNotice(
            t(event.busy ? "settingsHelp.helper.busy" : "settingsHelp.helper.declined", {
              name: nameOf(event.peer),
            })
          );
          break;
        case "proposed":
          setReceiving((prev) =>
            prev?.status === "active" ? { ...prev, question: { id: event.id, change: event.change } } : prev
          );
          settingsGet()
            .then(setOwnSettings)
            .catch((e) => console.error("Could not read the settings to ask about a change:", e));
          break;
        case "answered":
          setGiving((prev) => {
            if (prev?.status !== "active") return prev;
            const answer = event.answer;
            const name = nameOf(prev.peer);
            return {
              ...prev,
              waiting: prev.waiting === event.id ? null : prev.waiting,
              settings:
                answer.outcome === "applied" && (!prev.settings || answer.settings.revision >= prev.settings.revision)
                  ? answer.settings
                  : prev.settings,
              note:
                answer.outcome === "declined"
                  ? t("settingsHelp.helper.changeDeclined", { name })
                  : answer.outcome === "refused"
                    ? t(`settingsHelp.helper.refused.${answer.reason}`, { name })
                    : null,
            };
          });
          break;
        case "settings":
          setGiving((prev) =>
            prev?.status === "active" && (!prev.settings || event.settings.revision >= prev.settings.revision)
              ? { ...prev, settings: event.settings }
              : prev
          );
          break;
        case "ended": {
          const name = nameOf(event.peer);
          if (event.role === "helper") setGiving(null);
          else setReceiving(null);
          if (event.reason === "peer_stopped") setNotice(t("settingsHelp.ended.stopped", { name }));
          if (event.reason === "peer_left") setNotice(t("settingsHelp.ended.left", { name }));
          break;
        }
      }
    },
    [nameOf, t]
  );

  const offer = useCallback(
    (peerId: string) => {
      if (connId === null) return;
      settingsHelpRequest(connId, peerId)
        .then(() => setGiving({ status: "asking", peer: peerId }))
        .catch((e) => console.error("Could not offer help with settings:", e));
    },
    [connId]
  );

  const stop = useCallback(
    (role: "helper" | "helped") => {
      if (connId === null) return;
      if (role === "helper") setGiving(null);
      else setReceiving(null);
      settingsHelpStop(connId, role).catch((e) => console.error("Could not stop the help with settings:", e));
    },
    [connId]
  );

  const answerRequest = useCallback(
    (peer: string, accept: boolean) => {
      if (connId === null) return;
      // Declining starts nothing, so nothing will come back to clear the question.
      if (!accept) setReceiving(null);
      settingsHelpAnswer(connId, peer, accept).catch((e) => console.error("Could not answer the offer of help:", e));
    },
    [connId]
  );

  const decide = useCallback(
    (id: number, approve: boolean) => {
      if (connId === null) return;
      setReceiving((prev) =>
        prev?.status === "active" && prev.question?.id === id ? { ...prev, question: null } : prev
      );
      settingsHelpDecide(connId, id, approve).catch((e) => console.error("Could not answer the change:", e));
    },
    [connId]
  );

  const propose = useCallback(
    (change: SettingChange) => {
      if (connId === null) return;
      settingsHelpPropose(connId, change)
        .then((id) => setGiving((prev) => (prev?.status === "active" ? { ...prev, waiting: id, note: null } : prev)))
        .catch((e) => console.error("Could not propose the change:", e));
    },
    [connId]
  );

  const helpedTab = useAudioSettingsTab(giving?.status === "active" ? giving.settings : null, propose);

  const canOffer = useCallback(
    (peer: PeerInfo) => giving === null && (peer.features ?? []).includes(PEER_MESSAGE_FEATURE),
    [giving]
  );

  const bars: ReactNode[] = [];
  if (receiving?.status === "active") {
    bars.push(
      <SettingsHelpBar
        key="helped"
        message={t("settingsHelp.helped.bar", { name: receiving.name })}
        actions={[{ label: t("settingsHelp.helped.stop"), onClick: () => stop("helped"), testId: "settings-help-stop-helped" }]}
      />
    );
  }
  if (giving?.status === "asking") {
    bars.push(
      <SettingsHelpBar
        key="asking"
        message={t("settingsHelp.helper.asking", { name: nameOf(giving.peer) })}
        actions={[{ label: t("settingsHelp.helper.cancel"), onClick: () => stop("helper"), testId: "settings-help-cancel" }]}
      />
    );
  }
  if (giving?.status === "active") {
    const name = nameOf(giving.peer);
    const actions = [
      { label: t("settingsHelp.helper.stop"), onClick: () => stop("helper"), testId: "settings-help-stop-helper" },
    ];
    if (!giving.panelOpen) {
      actions.unshift({
        label: t("settingsHelp.helper.open"),
        onClick: () => setGiving((prev) => (prev?.status === "active" ? { ...prev, panelOpen: true } : prev)),
        testId: "settings-help-open",
      });
    }
    bars.push(<SettingsHelpBar key="giving" message={t("settingsHelp.helper.bar", { name })} actions={actions} />);
  }
  if (notice) {
    bars.push(<Toast key="notice" type="info" message={notice} />);
  }

  const pendingStatus =
    giving?.status === "active"
      ? giving.waiting !== null
        ? t("settingsHelp.helper.pending", { name: nameOf(giving.peer) })
        : giving.note
      : null;
  const question = receiving?.status === "active" ? receiving.question : null;

  const overlays = (
    <>
      <SettingsHelpQuestion
        open={receiving?.status === "asked"}
        questionKey={receiving?.peer}
        message={receiving ? t("settingsHelp.request.message", { name: receiving.name }) : ""}
        allowLabel={t("settingsHelp.request.allow")}
        declineLabel={t("settingsHelp.request.decline")}
        onAllow={() => receiving && answerRequest(receiving.peer, true)}
        onDecline={() => receiving && answerRequest(receiving.peer, false)}
      />
      <SettingsHelpQuestion
        open={question !== null}
        questionKey={question?.id}
        message={
          question && receiving
            ? t("settingsHelp.proposal.message", {
                name: receiving.name,
                setting: settingLabel(question.change.setting, t),
                value: changeValue(question.change, ownSettings, t),
              })
            : ""
        }
        allowLabel={t("settingsHelp.proposal.allow")}
        declineLabel={t("settingsHelp.proposal.decline")}
        onAllow={() => question && decide(question.id, true)}
        onDecline={() => question && decide(question.id, false)}
        stopLabel={t("settingsHelp.proposal.stop")}
        onStop={() => stop("helped")}
      />
      {/* Mounted only while helping, so nothing of the other person's settings stays behind. */}
      {giving?.status === "active" && (
        <SidePanel
          isOpen={giving.panelOpen}
          onClose={() => setGiving((prev) => (prev?.status === "active" ? { ...prev, panelOpen: false } : prev))}
          title={t("settingsHelp.helper.title", { name: nameOf(giving.peer) })}
        >
          <SettingsHelpPanel devicesTab={helpedTab} status={pendingStatus} waiting={giving.waiting !== null} />
        </SidePanel>
      )}
    </>
  );

  return { onEvent, canOffer, offer, bars: bars.length > 0 ? <div className="settings-help-bars">{bars}</div> : null, overlays };
}
