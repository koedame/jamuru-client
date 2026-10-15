/**
 * SidePanel component - Slide-in panel from the right side
 */
import { useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import "./SidePanel.css";

export interface SidePanelProps {
  isOpen: boolean;
  onClose: () => void;
  title?: string;
  /**
   * True (the default) for a panel that takes the screen over: a dimmed overlay,
   * no scrolling behind it, Escape closes it. False for a panel shown beside the
   * screen that is still being used.
   */
  modal?: boolean;
  children: React.ReactNode;
}

export function SidePanel({ isOpen, onClose, title, modal = true, children }: SidePanelProps) {
  const { t } = useTranslation();

  // Close on Escape key
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === "Escape" && isOpen && modal) {
        onClose();
      }
    },
    [isOpen, modal, onClose]
  );

  useEffect(() => {
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [handleKeyDown]);

  // Prevent body scroll when panel is open
  useEffect(() => {
    if (isOpen && modal) {
      document.body.style.overflow = "hidden";
    } else {
      document.body.style.overflow = "";
    }
    return () => {
      document.body.style.overflow = "";
    };
  }, [isOpen, modal]);

  return (
    <>
      {/* Overlay */}
      {modal && (
        <div
          className={`side-panel-overlay ${isOpen ? "side-panel-overlay--open" : ""}`}
          onClick={onClose}
          aria-hidden="true"
        />
      )}

      {/* Panel */}
      <aside
        className={`side-panel ${isOpen ? "side-panel--open" : ""}`}
        role="dialog"
        aria-modal={modal}
        aria-label={title}
      >
        <header className="side-panel__header">
          <h2 className="side-panel__title">{title}</h2>
          <button
            className="side-panel__close-btn"
            onClick={onClose}
            aria-label={t("common.button.close")}
          >
            <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          </button>
        </header>
        <div className="side-panel__content">
          {children}
        </div>
      </aside>
    </>
  );
}

export default SidePanel;
