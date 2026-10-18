/**
 * PeerLinkDetail component
 *
 * The layers the audio to and from one participant passes through, from this
 * machine's devices across the network to theirs. Each layer is that of one
 * link: with several participants, each has its own.
 */

import { useTranslation } from 'react-i18next';
import type { PeerLinkStatus } from '../../lib/tauri';
import { localBuffers } from '../../lib/links';
import './PeerLinkDetail.css';

export interface PeerLinkDetailProps {
  /** The participant's name */
  peerName: string;

  /** The audio link to them; undefined while there is none yet */
  link: PeerLinkStatus | undefined;

  /** This machine's sample rate in Hz, to tell whether their audio is resampled */
  localSampleRate: number;

  /** Id for the element, so the control that opens it can point at it */
  id?: string;
}

/**
 * Latency as the screen words it: one decimal below 10ms, where a rounded 0ms
 * would say nothing. Null for a value that is not a number: a participant's
 * app reports its own, and one that sends nonsense has no value to show.
 */
function formatMs(ms: number | null | undefined): string | null {
  if (ms == null || !Number.isFinite(ms)) return null;
  const tenths = Math.round(ms * 10) / 10;
  return tenths < 10 ? `${tenths.toFixed(1)}ms` : `${Math.round(ms)}ms`;
}

export function PeerLinkDetail({ peerName, link, localSampleRate, id }: PeerLinkDetailProps) {
  const { t } = useTranslation();

  const local = localBuffers(link?.latency);
  const network = link?.network ?? null;
  const rtt = network?.rtt_ms ?? null;
  const peer = link?.peer_audio ?? null;
  const measuring = t('session.participant.detail.measuring');

  const row = (label: string, value: string | null, testId: string) => (
    <div className="peer-link-detail__row">
      <dt>{label}</dt>
      <dd data-testid={testId}>{value ?? measuring}</dd>
    </div>
  );

  return (
    <div className="peer-link-detail" id={id} data-testid="peer-link-detail">
      <section className="peer-link-detail__layer" aria-label={t('session.participant.detail.you')}>
        <h4 className="peer-link-detail__title">{t('session.participant.detail.you')}</h4>
        <dl>
          {row(t('session.participant.detail.input'), formatMs(local?.input), 'detail-you-input')}
          {row(t('session.participant.detail.output'), formatMs(local?.output), 'detail-you-output')}
          {row(t('session.participant.detail.jitterBuffer'), formatMs(local?.jitter), 'detail-you-jitter')}
        </dl>
      </section>

      <section className="peer-link-detail__layer" aria-label={t('session.participant.detail.network')}>
        <h4 className="peer-link-detail__title">{t('session.participant.detail.network')}</h4>
        <dl>
          {row(t('session.participant.detail.rtt'), formatMs(rtt), 'detail-network-rtt')}
          {row(t('session.participant.detail.jitter'), formatMs(network?.jitter_ms), 'detail-network-jitter')}
          {row(
            t('session.participant.detail.packetLoss'),
            network && `${network.packet_loss_percent.toFixed(1)}%`,
            'detail-network-loss'
          )}
        </dl>
      </section>

      <section className="peer-link-detail__layer" aria-label={peerName}>
        <h4 className="peer-link-detail__title">{peerName}</h4>
        <dl>
          {row(t('session.participant.detail.input'), formatMs(peer?.capture_buffer_ms), 'detail-peer-input')}
          {row(t('session.participant.detail.output'), formatMs(peer?.playback_buffer_ms), 'detail-peer-output')}
          {row(t('session.participant.detail.jitterBuffer'), formatMs(peer?.jitter_buffer_ms), 'detail-peer-jitter')}
          {row(
            t('session.participant.detail.format'),
            peer &&
              t('session.participant.detail.formatValue', {
                rate: peer.sample_rate / 1000,
                frames: peer.frame_size,
                codec: peer.codec.toUpperCase(),
                channels: t(
                  peer.channel_count === 1
                    ? 'session.participant.detail.mono'
                    : 'session.participant.detail.stereo'
                ),
              }),
            'detail-peer-format'
          )}
        </dl>
        {peer?.needs_resampling && (
          <p className="peer-link-detail__warning" data-testid="detail-resampling">
            {t('sessionStats.resampling', {
              from: (peer.sample_rate / 1000).toFixed(1),
              to: (localSampleRate / 1000).toFixed(1),
            })}
          </p>
        )}
      </section>
    </div>
  );
}

export default PeerLinkDetail;
