import { useCallback, useEffect, useState, type ReactNode } from "react";
import { termsAccept, termsGet, type TermsInfo } from "../../lib/tauri";
import { ConsentScreen } from "./ConsentScreen";

/**
 * Shows `children` once the user has agreed to the terms of use in force, and
 * the consent screen until then. The backend does not connect, check for
 * updates or report usage until the agreement is recorded, so `children`
 * mount into an app that is still silent only if they are not rendered early.
 */
export function ConsentGate({ children }: { children: ReactNode }) {
  const [terms, setTerms] = useState<TermsInfo | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [accepting, setAccepting] = useState(false);
  const [acceptError, setAcceptError] = useState<string | null>(null);

  useEffect(() => {
    termsGet()
      .then(setTerms)
      .catch((e) => {
        console.error("Failed to load the terms of use:", e);
        setLoadError(String(e));
      });
  }, []);

  const handleAccept = useCallback(async () => {
    if (!terms) return;
    setAccepting(true);
    setAcceptError(null);
    try {
      await termsAccept(terms.version);
      setTerms({ ...terms, accepted: true });
    } catch (e) {
      console.error("Failed to record the agreement:", e);
      setAcceptError(String(e));
    } finally {
      setAccepting(false);
    }
  }, [terms]);

  if (loadError) {
    return (
      <p role="alert" data-testid="consent-load-error">
        {loadError}
      </p>
    );
  }
  if (!terms) return null;
  if (terms.accepted) return <>{children}</>;
  return (
    <ConsentScreen
      text={terms.text}
      onAccept={handleAccept}
      accepting={accepting}
      error={acceptError}
    />
  );
}
