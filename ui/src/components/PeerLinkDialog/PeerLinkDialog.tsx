/**
 * PeerLinkDialog component
 *
 * A participant's latency breakdown (PeerLinkDetail) in a dialog over the
 * screen, so the sidebar row stays one line. Opened by the user and closed by
 * the close button, Escape or a click outside; nothing else opens or closes it.
 */

import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { PeerLinkDetail, type PeerLinkDetailProps } from '../PeerLinkDetail';
import './PeerLinkDialog.css';

export interface PeerLinkDialogProps extends Omit<PeerLinkDetailProps, 'id'> {
  onClose: () => void;
}

export function PeerLinkDialog({ peerName, link, localSampleRate, onClose }: PeerLinkDialogProps) {
  const { t } = useTranslation();
  const closeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    closeRef.current?.focus();
  }, []);

  // The close button is the only control, so Tab stays on it rather than
  // reaching the screen behind the overlay.
  const handleKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      onClose();
    } else if (e.key === 'Tab') {
      e.preventDefault();
      closeRef.current?.focus();
    }
  };

  return (
    <div className="peer-link-dialog__overlay" data-testid="peer-link-dialog-overlay" onClick={onClose}>
      <div
        className="peer-link-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={peerName}
        data-testid="peer-link-dialog"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={handleKeyDown}
      >
        <header className="peer-link-dialog__header">
          <h3 className="peer-link-dialog__title">{peerName}</h3>
          <button
            ref={closeRef}
            type="button"
            className="peer-link-dialog__close"
            data-testid="peer-link-dialog-close"
            aria-label={t('common.button.close')}
            onClick={onClose}
          >
            ✕
          </button>
        </header>
        <PeerLinkDetail peerName={peerName} link={link} localSampleRate={localSampleRate} />
      </div>
    </div>
  );
}

export default PeerLinkDialog;
