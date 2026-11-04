/**
 * PeerBufferSection - the buffer one participant's audio is played from, and
 * the two ways to change it (ADR-097): the user holds it at a length of their
 * own, or asks the participant to hold the buffer their side plays the user's
 * audio from. Shown in the participant's dialog under the latency breakdown.
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MAX_BUFFER_MS10,
  MAX_TOLERANCE_TENTHS,
  type AskStanding,
  type BufferTerms,
  type PeerBuffer,
} from "../../lib/tauri";
import { msText, parseMs10, parseTenths } from "./format";
import "./PeerBufferSection.css";

export interface PeerBufferSectionProps {
  peerName: string;
  /** The buffer their audio is played from; undefined while there is no link yet */
  buffer: PeerBuffer | undefined;
  /** What the participant asked of the user's buffer for them, if the user took it up */
  held: { terms: BufferTerms } | null;
  /** The ask the user made of the participant, if one is shown */
  asked: { terms: BufferTerms; standing: AskStanding } | null;
  /** Whether the user can ask the participant (their app takes such asks) */
  canAsk: boolean;
  /** Whether the first-use explanation is shown */
  showHint: boolean;
  onDismissHint: () => void;
  /** Holds the buffer at this many tenths of a millisecond, or null for the preset's own */
  onSetOwn: (ms10: number | null) => void;
  /** Puts the buffer the participant asked for back to what it was */
  onRestoreHeld: () => void;
  onAsk: (terms: BufferTerms) => void;
  /** Takes the ask back, or clears what is shown of one that ended */
  onWithdraw: () => void;
}

/** An ask that is over: nothing more comes of it unless the user asks again */
function isOver(standing: AskStanding): boolean {
  return (
    standing.state === "declined" ||
    standing.state === "declined_by_setting" ||
    standing.state === "restored" ||
    standing.state === "unreachable"
  );
}

const DEFAULT_ASK_MS = "20";
const DEFAULT_ASK_TOLERANCE = "0";

export function PeerBufferSection({
  peerName,
  buffer,
  held,
  asked,
  canAsk,
  showHint,
  onDismissHint,
  onSetOwn,
  onRestoreHeld,
  onAsk,
  onWithdraw,
}: PeerBufferSectionProps) {
  const { t } = useTranslation();
  const [ownText, setOwnText] = useState(() =>
    buffer?.own.mode === "fixed" ? msText(buffer.own.ms10) : ""
  );
  const [askMs, setAskMs] = useState(DEFAULT_ASK_MS);
  const [askTolerance, setAskTolerance] = useState(DEFAULT_ASK_TOLERANCE);
  const [askAuto, setAskAuto] = useState(true);

  const own = buffer?.own ?? { mode: "preset" as const };
  const ownMs10 = parseMs10(ownText, MAX_BUFFER_MS10);
  const askMs10 = parseMs10(askMs, MAX_BUFFER_MS10);
  const tolerance = parseTenths(askTolerance, MAX_TOLERANCE_TENTHS);
  const askValid = askMs10 !== null && (!askAuto || tolerance !== null);

  const granted = buffer?.granted ?? null;
  const delay = buffer && buffer.delay_ms10 > 0 ? t("bufferRequest.ms", { ms: msText(buffer.delay_ms10) }) : null;
  const source =
    own.mode === "fixed"
      ? t("bufferRequest.source.held", { ms: msText(own.ms10) })
      : t("bufferRequest.source.preset");

  const standingText = (standing: AskStanding): string => {
    switch (standing.state) {
      case "sending":
        return t("bufferRequest.standing.sending", { name: peerName });
      case "pending":
        return t("bufferRequest.standing.pending", { name: peerName });
      case "granted": {
        const notes = [
          standing.applied.rounded ? t("bufferRequest.note.rounded") : null,
          standing.applied.at_lowest ? t("bufferRequest.note.atLowest") : null,
          standing.applied.auto_running ? t("bufferRequest.note.auto") : null,
        ].filter((note): note is string => note !== null);
        const base = t("bufferRequest.standing.granted", {
          name: peerName,
          ms: msText(standing.applied.applied_ms10),
        });
        return notes.length > 0 ? `${base} (${notes.join(t("bufferRequest.note.separator"))})` : base;
      }
      case "declined":
        return t("bufferRequest.standing.declined", { name: peerName });
      case "declined_by_setting":
        return t("bufferRequest.standing.declinedBySetting", { name: peerName });
      case "restored":
        return t("bufferRequest.standing.restored", { name: peerName });
      case "unreachable":
        return t("bufferRequest.standing.unreachable", { name: peerName });
    }
  };

  const asking = asked !== null && !isOver(asked.standing);

  return (
    <section className="peer-buffer" aria-label={t("bufferRequest.title")} data-testid="peer-buffer">
      <h4 className="peer-buffer__title">{t("bufferRequest.title")}</h4>

      {showHint && (
        <div className="peer-buffer__hint" role="note" data-testid="peer-buffer-hint">
          <p>{t("bufferRequest.hint", { name: peerName })}</p>
          <button type="button" className="peer-buffer__button" onClick={onDismissHint} data-testid="peer-buffer-hint-dismiss">
            {t("bufferRequest.hintDismiss")}
          </button>
        </div>
      )}

      <dl className="peer-buffer__rows">
        <div className="peer-buffer__row">
          <dt>{t("bufferRequest.now")}</dt>
          <dd data-testid="peer-buffer-now">{delay ?? t("session.participant.detail.measuring")}</dd>
        </div>
        <div className="peer-buffer__row">
          <dt>{t("bufferRequest.setBy")}</dt>
          <dd data-testid="peer-buffer-source">{source}</dd>
        </div>
        {granted && (
          <div className="peer-buffer__row">
            <dt>{t("bufferRequest.raisedBy")}</dt>
            <dd data-testid="peer-buffer-raised-by">{t("bufferRequest.raisedByValue", { name: peerName })}</dd>
          </div>
        )}
      </dl>

      <div className="peer-buffer__field">
        <label htmlFor="peer-buffer-own">{t("bufferRequest.holdAt")}</label>
        <div className="peer-buffer__controls">
          <input
            id="peer-buffer-own"
            className="peer-buffer__input"
            type="text"
            inputMode="decimal"
            value={ownText}
            placeholder={own.mode === "fixed" ? undefined : t("bufferRequest.presetPlaceholder")}
            onChange={(e) => setOwnText(e.target.value)}
            data-testid="peer-buffer-own"
          />
          <span className="peer-buffer__unit">{t("bufferRequest.unit")}</span>
          <button
            type="button"
            className="peer-buffer__button"
            disabled={ownMs10 === null}
            onClick={() => ownMs10 !== null && onSetOwn(ownMs10)}
            data-testid="peer-buffer-own-set"
          >
            {t("bufferRequest.set")}
          </button>
          {own.mode === "fixed" && (
            <button
              type="button"
              className="peer-buffer__button"
              onClick={() => {
                setOwnText("");
                onSetOwn(null);
              }}
              data-testid="peer-buffer-own-preset"
            >
              {t("bufferRequest.usePreset")}
            </button>
          )}
        </div>
        <p className="peer-buffer__hint-line">{t("bufferRequest.holdHint", { max: msText(MAX_BUFFER_MS10) })}</p>
      </div>

      {held && (
        <div className="peer-buffer__held" data-testid="peer-buffer-held">
          <p>
            {t(held.terms.auto ? "bufferRequest.held.auto" : "bufferRequest.held.fixed", {
              name: peerName,
              ms: msText(granted?.applied_ms10 ?? held.terms.max_ms10),
              max: msText(held.terms.max_ms10),
            })}
          </p>
          <button type="button" className="peer-buffer__button" onClick={onRestoreHeld} data-testid="peer-buffer-restore">
            {t("bufferRequest.held.restore")}
          </button>
        </div>
      )}

      {canAsk && (
        <div className="peer-buffer__ask" data-testid="peer-buffer-ask">
          <h5 className="peer-buffer__subtitle">{t("bufferRequest.ask.title", { name: peerName })}</h5>
          <p className="peer-buffer__hint-line">{t("bufferRequest.ask.what", { name: peerName })}</p>
          {asked && (
            <p className="peer-buffer__standing" role="status" data-testid="peer-buffer-standing" data-state={asked.standing.state}>
              {standingText(asked.standing)}
            </p>
          )}
          {asking ? (
            <button type="button" className="peer-buffer__button" onClick={onWithdraw} data-testid="peer-buffer-withdraw">
              {t("bufferRequest.ask.withdraw")}
            </button>
          ) : (
            <>
              <div className="peer-buffer__field">
                <label htmlFor="peer-buffer-ask-ms">{t("bufferRequest.ask.longest")}</label>
                <div className="peer-buffer__controls">
                  <input
                    id="peer-buffer-ask-ms"
                    className="peer-buffer__input"
                    type="text"
                    inputMode="decimal"
                    value={askMs}
                    onChange={(e) => setAskMs(e.target.value)}
                    data-testid="peer-buffer-ask-ms"
                  />
                  <span className="peer-buffer__unit">{t("bufferRequest.unit")}</span>
                </div>
              </div>
              <label className="peer-buffer__check">
                <input
                  type="checkbox"
                  checked={askAuto}
                  onChange={(e) => setAskAuto(e.target.checked)}
                  data-testid="peer-buffer-ask-auto"
                />
                <span>{t("bufferRequest.ask.auto")}</span>
              </label>
              {askAuto && (
                <div className="peer-buffer__field">
                  <label htmlFor="peer-buffer-ask-tolerance">{t("bufferRequest.ask.tolerance")}</label>
                  <div className="peer-buffer__controls">
                    <input
                      id="peer-buffer-ask-tolerance"
                      className="peer-buffer__input"
                      type="text"
                      inputMode="decimal"
                      value={askTolerance}
                      onChange={(e) => setAskTolerance(e.target.value)}
                      data-testid="peer-buffer-ask-tolerance"
                    />
                    <span className="peer-buffer__unit">{t("bufferRequest.ask.perSecond")}</span>
                  </div>
                  <p className="peer-buffer__hint-line">{t("bufferRequest.ask.toleranceHint", { max: MAX_TOLERANCE_TENTHS / 10 })}</p>
                </div>
              )}
              <button
                type="button"
                className="peer-buffer__button peer-buffer__button--primary"
                disabled={!askValid}
                onClick={() =>
                  askMs10 !== null &&
                  onAsk({ max_ms10: askMs10, tolerance_tenths: askAuto ? (tolerance ?? 0) : 0, auto: askAuto })
                }
                data-testid="peer-buffer-ask-send"
              >
                {asked ? t("bufferRequest.ask.askAgain") : t("bufferRequest.ask.send")}
              </button>
            </>
          )}
        </div>
      )}
    </section>
  );
}

export default PeerBufferSection;
