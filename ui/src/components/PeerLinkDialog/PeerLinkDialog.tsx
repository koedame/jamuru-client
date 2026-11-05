/**
 * PeerLinkDialog component
 *
 * A participant's latency breakdown (PeerLinkDetail) and receive buffer
 * (PeerBufferSection) in a dialog over the
 * screen, so the sidebar row stays one line. Opened by the user and closed by
 * the close button, Escape or a click outside; nothing else opens or closes it.
 */

import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { streamingSetPeerBuffer } from '../../lib/tauri';
import { PeerLinkDetail, type PeerLinkDetailProps } from '../PeerLinkDetail';
import { PeerBufferSection, useFirstUseHint, type BufferRequests } from '../BufferRequest';
import './PeerLinkDialog.css';

/** Remembers that the explanation of the buffer controls has been shown */
const BUFFER_HINT_KEY = 'jamuru.hint.peerBuffer';

export interface PeerLinkDialogProps extends Omit<PeerLinkDetailProps, 'id'> {
  /** The participant's id in the room */
  peerId: string;
  /** Whether the participant's app takes asks to buffer longer (ADR-097) */
  askable: boolean;
  /** The asks made and received, and what is done with them; without it the buffer is not offered (a helper's window) */
  requests?: BufferRequests;
  onClose: () => void;
}

export function PeerLinkDialog({
  peerId,
  peerName,
  link,
  localSampleRate,
  askable,
  requests,
  onClose,
}: PeerLinkDialogProps) {
  const { t } = useTranslation();
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const hint = useFirstUseHint(BUFFER_HINT_KEY, requests !== undefined);

  useEffect(() => {
    closeRef.current?.focus();
  }, []);

  // Tab goes round the dialog's own controls rather than reaching the screen
  // behind the overlay.
  const handleKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      onClose();
      return;
    }
    if (e.key !== 'Tab') return;
    const controls = Array.from(
      dialogRef.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)') ?? []
    );
    const first = controls[0];
    const last = controls[controls.length - 1];
    if (!first || !last) return;
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  const reportFailure = (what: string) => (e: unknown) => console.error(`Could not ${what}:`, e);

  return (
    <div className="peer-link-dialog__overlay" data-testid="peer-link-dialog-overlay" onClick={onClose}>
      <div
        ref={dialogRef}
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
        {requests && (
          <PeerBufferSection
            peerName={peerName}
            buffer={link?.buffer}
            held={requests.heldFor(peerId)}
            asked={requests.askedOf(peerId)}
            canAsk={askable && requests.canAsk}
            showHint={hint.visible}
            onDismissHint={hint.dismiss}
            onSetOwn={(ms10) => streamingSetPeerBuffer(peerId, ms10).catch(reportFailure('set the buffer'))}
            onRestoreHeld={() => requests.restore(peerId).catch(reportFailure('put the buffer back'))}
            onAsk={(terms) => requests.ask(peerId, terms).catch(reportFailure('ask for a longer buffer'))}
            onWithdraw={() => requests.withdraw(peerId).catch(reportFailure('take the ask back'))}
          />
        )}
      </div>
    </div>
  );
}

export default PeerLinkDialog;
