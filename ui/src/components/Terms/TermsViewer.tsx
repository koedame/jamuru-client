import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { TermsText } from "./TermsText";
import "./Terms.css";

export interface TermsViewerProps {
  title: string;
  text: string;
  format?: "markdown" | "plain";
  onClose: () => void;
}

/** A dialog that shows the terms of use or the license to read. Escape closes it. */
export function TermsViewer({ title, text, format, onClose }: TermsViewerProps) {
  const { t } = useTranslation();
  const closeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    closeRef.current?.focus();
  }, []);

  const handleKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") onClose();
  };

  return (
    <div className="terms-viewer__overlay" onClick={onClose}>
      <div
        className="terms-viewer"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={handleKeyDown}
      >
        <h2 className="terms-viewer__title">{title}</h2>
        <div className="terms-viewer__body">
          <TermsText text={text} format={format} />
        </div>
        <button ref={closeRef} type="button" className="terms-viewer__close" onClick={onClose}>
          {t("common.button.close")}
        </button>
      </div>
    </div>
  );
}
