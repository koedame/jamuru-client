/**
 * Asking participants to buffer your audio for longer, and answering their
 * asks (ADR-097): the state of the asks this app made and was made, built from
 * the events the backend sends, and what to show for it - the question the
 * user answers and the passing notices. Where an ask is made and where a
 * buffer held for someone is put back is the participant's dialog
 * (`PeerLinkDialog`), which reads and calls what this returns.
 *
 * An ask is for this session only: the backend ends it when the asker or this
 * app leaves, so nothing here is kept.
 */
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useWindowEvent } from "../../hooks/useWindowEvents";
import {
  BUFFER_REQUEST_EVENT,
  bufferRequestAnswer,
  bufferRequestAsk,
  bufferRequestRestore,
  bufferRequestSnapshot,
  bufferRequestWithdraw,
  type AskStanding,
  type BufferRequestEvent,
  type BufferTerms,
  type GrantView,
  type Participant,
} from "../../lib/tauri";
import { Toast } from "../Toast";
import { SettingsHelpQuestion } from "../SettingsHelp";
import { msText } from "./format";

/** An ask this app made of a participant */
export interface AskedOf {
  terms: BufferTerms;
  standing: AskStanding;
}

/** A buffer this app holds because a participant asked */
export interface HeldFor {
  terms: BufferTerms;
  /** How it stands on the audio; null until the audio thread has said */
  applied: GrantView | null;
}

/** An ask waiting for the user. `serial` makes a new ask of the same person a new question. */
interface Prompt {
  peer: string;
  name: string;
  terms: BufferTerms;
  serial: number;
}

/** How long a passing notice stays up */
const NOTICE_MS = 6000;

export interface BufferRequests {
  /** The ask this app made of `peer`, if one is shown */
  askedOf: (peer: string) => AskedOf | null;
  /** The buffer this app holds for `peer`, if it does */
  heldFor: (peer: string) => HeldFor | null;
  /** Whether asks can be made now: there is a room connection */
  canAsk: boolean;
  ask: (peer: string, terms: BufferTerms) => Promise<void>;
  /** Takes the ask back, or clears what is shown of one that ended */
  withdraw: (peer: string) => Promise<void>;
  /** Puts the buffer held for `peer` back to what it was */
  restore: (peer: string) => Promise<void>;
  /** The passing notice, for the app's toast stack */
  toast: ReactNode;
  /** The question of whether to take an ask, drawn over the screen */
  overlays: ReactNode;
}

export function useBufferRequests(connId: number | null, participants: Participant[]): BufferRequests {
  const { t } = useTranslation();
  const [asked, setAsked] = useState<Record<string, AskedOf>>({});
  const [held, setHeld] = useState<Record<string, HeldFor>>({});
  const [prompts, setPrompts] = useState<Prompt[]>([]);
  const [notice, setNotice] = useState<string | null>(null);

  const participantsRef = useRef(participants);
  participantsRef.current = participants;
  const nameOf = useCallback(
    (peer: string) => participantsRef.current.find((p) => p.id === peer)?.name ?? "",
    []
  );
  // The terms of an ask, as last known, for the notice about how it ended.
  const askedRef = useRef(asked);
  askedRef.current = asked;
  const serial = useRef(0);

  useEffect(() => {
    if (notice === null) return;
    const timer = setTimeout(() => setNotice(null), NOTICE_MS);
    return () => clearTimeout(timer);
  }, [notice]);

  // A window that opens after asks began, or a connection that is new, starts
  // from how the asks stand.
  useEffect(() => {
    if (connId === null) {
      setAsked({});
      setHeld({});
      setPrompts([]);
      return;
    }
    let cancelled = false;
    bufferRequestSnapshot()
      .then((snapshot) => {
        if (cancelled) return;
        setAsked(Object.fromEntries(snapshot.asked.map((a) => [a.peer_id, { terms: a.terms, standing: a.standing }])));
        setHeld(Object.fromEntries(snapshot.held.map((h) => [h.peer_id, { terms: h.terms, applied: h.applied }])));
        setPrompts(
          snapshot.prompts.map((p) => ({ peer: p.peer_id, name: p.peer_name, terms: p.terms, serial: ++serial.current }))
        );
      })
      .catch((e) => console.error("Could not read where the buffer asks stand:", e));
    return () => {
      cancelled = true;
    };
  }, [connId]);

  const onStanding = useCallback(
    (peer: string, standing: AskStanding | null) => {
      const previous = askedRef.current[peer]?.standing.state;
      if (standing === null) {
        setAsked((prev) => {
          const { [peer]: _gone, ...rest } = prev;
          return rest;
        });
        return;
      }
      setAsked((prev) => (prev[peer] ? { ...prev, [peer]: { ...prev[peer], standing } } : prev));
      if (standing.state === previous) return;
      const name = nameOf(peer);
      switch (standing.state) {
        case "granted":
          setNotice(t("bufferRequest.notice.granted", { name, ms: msText(standing.applied.applied_ms10) }));
          break;
        case "declined":
          setNotice(t("bufferRequest.notice.declined", { name }));
          break;
        case "declined_by_setting":
          setNotice(t("bufferRequest.notice.declinedBySetting", { name }));
          break;
        case "restored":
          setNotice(t("bufferRequest.notice.restored", { name }));
          break;
        case "unreachable":
          setNotice(t("bufferRequest.notice.unreachable", { name }));
          break;
      }
    },
    [nameOf, t]
  );

  const onEvent = useCallback(
    (event: BufferRequestEvent) => {
      switch (event.type) {
        case "asked":
          setPrompts((prev) => [
            ...prev.filter((p) => p.peer !== event.peer),
            { peer: event.peer, name: event.peer_name, terms: event.terms, serial: ++serial.current },
          ]);
          break;
        case "ask_gone":
          setPrompts((prev) => prev.filter((p) => p.peer !== event.peer));
          break;
        case "held":
          setPrompts((prev) => prev.filter((p) => p.peer !== event.peer));
          setHeld((prev) => ({ ...prev, [event.peer]: { terms: event.terms, applied: event.applied } }));
          break;
        case "released":
          setPrompts((prev) => prev.filter((p) => p.peer !== event.peer));
          setHeld((prev) => {
            const { [event.peer]: _gone, ...rest } = prev;
            return rest;
          });
          if (event.reason === "withdrawn") {
            setNotice(t("bufferRequest.notice.withdrawn", { name: nameOf(event.peer) }));
          }
          break;
        case "standing":
          onStanding(event.peer, event.standing);
          break;
      }
    },
    [nameOf, onStanding, t]
  );
  const onEventRef = useRef(onEvent);
  onEventRef.current = onEvent;
  const handleEvent = useCallback((event: BufferRequestEvent) => onEventRef.current(event), []);
  useWindowEvent<BufferRequestEvent>(BUFFER_REQUEST_EVENT, handleEvent);

  const ask = useCallback(
    async (peer: string, terms: BufferTerms) => {
      if (connId === null) return;
      await bufferRequestAsk(connId, peer, terms);
      setAsked((prev) => ({ ...prev, [peer]: { terms, standing: { state: "sending" } } }));
    },
    [connId]
  );

  const withdraw = useCallback(
    async (peer: string) => {
      if (connId === null) return;
      await bufferRequestWithdraw(connId, peer);
      setAsked((prev) => {
        const { [peer]: _gone, ...rest } = prev;
        return rest;
      });
    },
    [connId]
  );

  const restore = useCallback(
    async (peer: string) => {
      if (connId === null) return;
      await bufferRequestRestore(connId, peer);
      setHeld((prev) => {
        const { [peer]: _gone, ...rest } = prev;
        return rest;
      });
    },
    [connId]
  );

  const answer = useCallback(
    (peer: string, accept: boolean) => {
      setPrompts((prev) => prev.filter((p) => p.peer !== peer));
      if (connId === null) return;
      bufferRequestAnswer(connId, peer, accept).catch((e) => console.error("Could not answer the buffer ask:", e));
    },
    [connId]
  );

  const prompt = prompts[0];
  const overlays = prompt ? (
    <SettingsHelpQuestion
      key={`buffer-ask-${prompt.serial}`}
      open
      message={t("bufferRequest.question.message", { name: prompt.name, ms: msText(prompt.terms.max_ms10) })}
      detail={t(
        prompt.terms.auto ? "bufferRequest.question.detailAuto" : "bufferRequest.question.detailFixed",
        { name: prompt.name, ms: msText(prompt.terms.max_ms10) }
      )}
      allowLabel={t("bufferRequest.question.accept")}
      declineLabel={t("bufferRequest.question.decline")}
      onAllow={() => answer(prompt.peer, true)}
      onDecline={() => answer(prompt.peer, false)}
    />
  ) : null;

  return {
    askedOf: (peer) => asked[peer] ?? null,
    heldFor: (peer) => held[peer] ?? null,
    canAsk: connId !== null,
    ask,
    withdraw,
    restore,
    toast: notice ? <Toast type="info" message={notice} /> : null,
    overlays,
  };
}
