/**
 * Helping with settings in the main window (ADR-044 §5): the state of the help
 * this app gives and receives, built from the events the backend sends, and what
 * to show for it - the one question the helped side answers, and the bar that
 * shows help is going on (with Stop).
 *
 * The help itself is not drawn here. The person helping works in a window of
 * their own that shows the helped app's screen (`HelperScreen`); the person
 * helped keeps working in this one, with the bar to stop it whenever they like.
 */
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  PEER_MESSAGE_FEATURE,
  settingsHelpAnswer,
  settingsHelpRequest,
  settingsHelpStop,
  type HelpEvent,
  type Participant,
} from "../../lib/tauri";
import { Toast } from "../Toast";
import { SettingsHelpBar } from "./SettingsHelpBar";
import { SettingsHelpQuestion } from "./SettingsHelpQuestion";

/**
 * This app helping someone. `name` is kept from when the help began, so it
 * can still be said after they leave the participant list.
 */
type Giving = { status: "asking" | "active"; peer: string; name: string };

/**
 * Someone helping this app. `serial` numbers each request this app was
 * asked, so a question for a new request is a new question even from the
 * same person.
 */
type Receiving =
  | { status: "asked"; peer: string; name: string; serial: number }
  /** Allowed; the help begins once the app has opened its end of the connection */
  | { status: "starting"; peer: string; name: string }
  | { status: "active"; peer: string; name: string };

/** How long a passing notice (declined, ended) stays up */
const NOTICE_MS = 6000;

export interface SettingsHelp {
  /** Hand every SettingsHelp event the backend sends to this */
  onEvent: (event: HelpEvent) => void;
  /** Whether this app can offer `peer` help now */
  canOffer: (peer: Participant) => boolean;
  offer: (peerId: string) => void;
  /** The bars for help going on, for the session screen */
  bars: ReactNode;
  /** The question of whether to allow a help, drawn over the screen */
  overlays: ReactNode;
}

export function useSettingsHelp(connId: number | null, participants: Participant[]): SettingsHelp {
  const { t } = useTranslation();
  const [giving, setGiving] = useState<Giving | null>(null);
  const [receiving, setReceiving] = useState<Receiving | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

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
  const requests = useRef(0);

  useEffect(() => {
    if (notice === null) return;
    const timer = setTimeout(() => setNotice(null), NOTICE_MS);
    return () => clearTimeout(timer);
  }, [notice]);

  const onEvent = useCallback(
    (event: HelpEvent) => {
      switch (event.type) {
        case "requested":
          requests.current += 1;
          setReceiving({ status: "asked", peer: event.peer, name: event.peer_name, serial: requests.current });
          break;
        case "started":
          if (event.role === "helper") {
            setGiving({ status: "active", peer: event.peer, name: event.peer_name });
          } else {
            setReceiving({ status: "active", peer: event.peer, name: event.peer_name });
          }
          break;
        case "declined":
          setGiving(null);
          setNotice(
            t(event.busy ? "settingsHelp.helper.busy" : "settingsHelp.helper.declined", {
              name: givingRef.current?.name || nameOf(event.peer),
            })
          );
          break;
        case "ended": {
          // Every end describes the app's help as it is now (stopping here
          // clears the help without one), so it ends the help with that peer.
          const current = event.role === "helper" ? givingRef.current : receivingRef.current;
          if (event.role === "helper") setGiving((prev) => (prev?.peer === event.peer ? null : prev));
          else setReceiving((prev) => (prev?.peer === event.peer ? null : prev));
          const name = (current?.peer === event.peer ? current.name : "") || nameOf(event.peer);
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
      const name = nameOf(peerId);
      settingsHelpRequest(connId, peerId)
        .then(() => setGiving({ status: "asking", peer: peerId, name }))
        .catch((e) => console.error("Could not offer help with settings:", e));
    },
    [connId, nameOf]
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
      // Allowing takes the app a moment (it opens its end of the connection), so
      // the question closes now and the help is shown when it has started.
      setReceiving((prev) =>
        accept && prev?.status === "asked" ? { status: "starting", peer: prev.peer, name: prev.name } : null
      );
      settingsHelpAnswer(connId, peer, accept).catch((e) => {
        console.error("Could not answer the offer of help:", e);
        // The help did not start, so nothing else will end the wait.
        setReceiving((prev) => (prev?.status === "starting" && prev.peer === peer ? null : prev));
      });
    },
    [connId]
  );

  const canOffer = useCallback(
    (peer: Participant) => giving === null && (peer.features ?? []).includes(PEER_MESSAGE_FEATURE),
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
        message={t("settingsHelp.helper.asking", { name: giving.name })}
        actions={[{ label: t("settingsHelp.helper.cancel"), onClick: () => stop("helper"), testId: "settings-help-cancel" }]}
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
      {receiving?.status === "asked" && (
        <SettingsHelpQuestion
          key={`request-${receiving.serial}`}
          open
          message={t("settingsHelp.request.message", { name: receiving.name })}
          allowLabel={t("settingsHelp.request.allow")}
          declineLabel={t("settingsHelp.request.decline")}
          onAllow={() => answerRequest(receiving.peer, true)}
          onDecline={() => answerRequest(receiving.peer, false)}
        />
      )}
    </>
  );

  return { onEvent, canOffer, offer, bars: bars.length > 0 ? <div className="settings-help-bars">{bars}</div> : null, overlays };
}
