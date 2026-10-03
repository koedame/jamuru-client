import { useState } from "react";
import { useTranslation } from "react-i18next";
import { TermsText } from "./TermsText";
import "./Terms.css";

export interface ConsentScreenProps {
  /** The terms of use, in Markdown */
  text: string;
  /** Called when the user agrees. Only reachable with both boxes checked */
  onAccept: () => void;
  /** True while the agreement is being saved */
  accepting?: boolean;
  /** Why the agreement could not be saved */
  error?: string | null;
}

/**
 * The first screen of a first launch (and after the terms change): the terms
 * of use, two boxes and one button. Nothing is sent to the server until the
 * user has agreed.
 */
export function ConsentScreen({ text, onAccept, accepting = false, error = null }: ConsentScreenProps) {
  const { t } = useTranslation();
  const [agreed, setAgreed] = useState(false);
  const [adult, setAdult] = useState(false);

  return (
    <main className="consent" data-testid="consent-screen">
      <h1 className="consent__title">{t("terms.consent.title")}</h1>
      <p className="consent__notice" data-testid="consent-ip-notice">
        {t("terms.consent.ipNotice")}
      </p>
      <section className="consent__text" aria-label={t("terms.consent.textLabel")} tabIndex={0}>
        <TermsText text={text} />
      </section>
      <label className="consent__check">
        <input
          type="checkbox"
          checked={agreed}
          onChange={(e) => setAgreed(e.target.checked)}
          data-testid="consent-agree"
        />
        <span>{t("terms.consent.agree")}</span>
      </label>
      <label className="consent__check">
        <input
          type="checkbox"
          checked={adult}
          onChange={(e) => setAdult(e.target.checked)}
          data-testid="consent-adult"
        />
        <span>{t("terms.consent.adult")}</span>
      </label>
      {error && (
        <p className="consent__error" role="alert" data-testid="consent-error">
          {t("terms.consent.error", { message: error })}
        </p>
      )}
      <button
        type="button"
        className="consent__start"
        disabled={!(agreed && adult) || accepting}
        onClick={onAccept}
        data-testid="consent-start"
      >
        {accepting ? t("terms.consent.accepting") : t("terms.consent.start")}
      </button>
      <p className="consent__decline">{t("terms.consent.decline")}</p>
    </main>
  );
}
