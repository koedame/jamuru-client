/**
 * DropoutNotice - says a participant's audio stopped without a leave notice, for how long, and
 * whose line is cut (REQ-CON-161, REQ-CON-163). Shown in the roster row and on the mixer strip.
 */

import { useTranslation } from "react-i18next";
import type { PeerDropout } from "../../lib/tauri";
import "./DropoutNotice.css";

export interface DropoutNoticeProps {
  /** Set while nothing arrives from the participant and they have not left */
  dropout: PeerDropout | null;
  /** Went quiet twice or more in the last quarter of an hour */
  unstable: boolean;
  /** The roster row runs the notice along a line; the mixer strip stacks it in a narrow column */
  variant: "row" | "strip";
}

export function DropoutNotice({ dropout, unstable, variant }: DropoutNoticeProps) {
  const { t } = useTranslation();
  if (!dropout && !unstable) return null;
  // "unknown" says nothing more than that the audio stopped
  const side = dropout && dropout.side !== "unknown" ? dropout.side : null;
  return (
    <div
      className={`dropout-notice dropout-notice--${variant}`}
      data-testid="dropout-notice"
      data-side={dropout?.side}
    >
      {dropout && (
        <span className="dropout-notice__alert" role="status">
          <span className="dropout-notice__heading" data-testid="dropout-heading">
            {t("dropout.heading")}
          </span>
          {/* Counting up every second: kept out of what a screen reader announces */}
          <span className="dropout-notice__seconds" data-testid="dropout-seconds" aria-hidden="true">
            {t("dropout.seconds", { seconds: dropout.silent_secs })}
          </span>
          {side && (
            <span className="dropout-notice__side" data-testid="dropout-side">
              {t(`dropout.side.${side}`)}
            </span>
          )}
        </span>
      )}
      {unstable && (
        <span
          className="dropout-notice__unstable"
          data-testid="dropout-unstable"
          title={t("dropout.unstableHint")}
        >
          {t("dropout.unstable")}
        </span>
      )}
    </div>
  );
}
