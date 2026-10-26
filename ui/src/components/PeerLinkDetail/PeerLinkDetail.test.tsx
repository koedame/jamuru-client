/**
 * PeerLinkDetail component tests
 */

import { beforeAll, describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';
import i18n from '../../i18n';
import type { PeerAudioInfo, PeerLinkStatus } from '../../lib/tauri';
import { PeerLinkDetail } from './PeerLinkDetail';

beforeAll(async () => {
  await i18n.changeLanguage('en');
});

const PEER_AUDIO: PeerAudioInfo = {
  sample_rate: 48000,
  frame_size: 64,
  codec: 'pcm',
  needs_resampling: false,
  channel_count: 2,
  capture_buffer_ms: 1.33,
  playback_buffer_ms: 2.67,
  jitter_buffer_ms: 5,
};

/** A link that is up, with this machine at 0.67ms in and 1ms out and a 2ms jitter buffer */
function upLink(changes: Partial<PeerLinkStatus> = {}): PeerLinkStatus {
  return {
    peer_id: 'b',
    connection_state: 'connected',
    connection_error: null,
    network: {
      rtt_ms: 12.4,
      jitter_ms: 0.8,
      packet_loss_percent: 0.5,
      quality: 'good',
      measured_bps: 0,
      required_bps: 0,
      bandwidth_status: 'sufficient',
      uptime_seconds: 0,
      packets_sent: 10,
      packets_received: 10,
      encryption: 'encrypted',
      bytes_sent: 0,
      bytes_received: 0,
    },
    latency: {
      upstream: [
        { name: 'Capture buffer', ms: 0.67, info: null },
        { name: 'Encode (pcm)', ms: 0, info: null },
        { name: 'Network', ms: 6.2, info: null },
      ],
      upstream_total_ms: 6.87,
      downstream: [
        { name: 'Network', ms: 6.2, info: null },
        { name: 'Jitter buffer', ms: 2, info: null },
        { name: 'Decode (pcm)', ms: 0, info: null },
        { name: 'Playback buffer', ms: 1, info: null },
      ],
      downstream_total_ms: 9.2,
      roundtrip_total_ms: 16.07,
    },
    peer_audio: PEER_AUDIO,
    level: 0,
    ...changes,
  };
}

const text = (testId: string) => screen.getByTestId(testId).textContent;

describe('PeerLinkDetail', () => {
  // Verifies: REQ-LAT-122
  it('リンクが上がっているとき、自分の入力・出力・ジッターバッファの遅延を層ごとに出すこと', () => {
    render(<PeerLinkDetail peerName="Aki" link={upLink()} localSampleRate={48000} />);

    expect(text('detail-you-input')).toBe('0.7ms');
    expect(text('detail-you-output')).toBe('1.0ms');
    expect(text('detail-you-jitter')).toBe('2.0ms');
  });

  it('リンクが上がっているとき、その人とのネットワークの往復・ジッター・パケットロスを出すこと', () => {
    render(<PeerLinkDetail peerName="Aki" link={upLink()} localSampleRate={48000} />);

    expect(text('detail-network-rtt')).toBe('12ms');
    expect(text('detail-network-jitter')).toBe('0.8ms');
    expect(text('detail-network-loss')).toBe('0.5%');
  });

  it('相手の遅延を聞いているとき、相手側の入力・出力・ジッターバッファと音声の形式を出すこと', () => {
    render(<PeerLinkDetail peerName="Aki" link={upLink()} localSampleRate={48000} />);

    expect(screen.getByRole('region', { name: 'Aki' })).toBeInTheDocument();
    expect(text('detail-peer-input')).toBe('1.3ms');
    expect(text('detail-peer-output')).toBe('2.7ms');
    expect(text('detail-peer-jitter')).toBe('5.0ms');
    expect(text('detail-peer-format')).toBe('48kHz, 64 samples, PCM, Stereo');
  });

  it('自分と相手のサンプルレートが違うとき、リサンプリングしていると知らせること', () => {
    const link = upLink({ peer_audio: { ...PEER_AUDIO, sample_rate: 44100, needs_resampling: true } });

    render(<PeerLinkDetail peerName="Aki" link={link} localSampleRate={48000} />);

    expect(text('detail-resampling')).toContain('44.1');
    expect(text('detail-resampling')).toContain('48.0');
  });

  it('サンプルレートが同じとき、リサンプリングとは知らせないこと', () => {
    render(<PeerLinkDetail peerName="Aki" link={upLink()} localSampleRate={48000} />);

    expect(screen.queryByTestId('detail-resampling')).not.toBeInTheDocument();
  });

  it('リンクがまだ無いとき、どの値も測定中と出し、数値を出さないこと', () => {
    render(<PeerLinkDetail peerName="Aki" link={undefined} localSampleRate={48000} />);

    for (const id of [
      'detail-you-input',
      'detail-network-rtt',
      'detail-peer-input',
      'detail-peer-format',
    ]) {
      expect(text(id)).toBe('Measuring...');
    }
  });

  // Verifies: REQ-LAT-130
  it('往復をまだ測っていないとき、往復を測定中と出すこと', () => {
    const link = upLink();
    link.network!.rtt_ms = null;

    render(<PeerLinkDetail peerName="Aki" link={link} localSampleRate={48000} />);

    expect(text('detail-network-rtt')).toBe('Measuring...');
  });

  it('10ms 以上のとき、小数を出さずに丸めて出すこと', () => {
    const link = upLink();
    link.network!.rtt_ms = 23.6;

    render(<PeerLinkDetail peerName="Aki" link={link} localSampleRate={48000} />);

    expect(text('detail-network-rtt')).toBe('24ms');
  });

  it('9.95ms のとき、10.0ms ではなく 10ms と出すこと', () => {
    const link = upLink();
    link.network!.rtt_ms = 9.96;

    render(<PeerLinkDetail peerName="Aki" link={link} localSampleRate={48000} />);

    expect(text('detail-network-rtt')).toBe('10ms');
  });

  it('相手の知らせた遅延が数値でないとき、落ちずに測定中と出すこと', () => {
    // A NaN the participant's app sent arrives as null
    const told = { ...PEER_AUDIO, capture_buffer_ms: null, playback_buffer_ms: Infinity } as unknown as PeerAudioInfo;

    render(<PeerLinkDetail peerName="Aki" link={upLink({ peer_audio: told })} localSampleRate={48000} />);

    expect(text('detail-peer-input')).toBe('Measuring...');
    expect(text('detail-peer-output')).toBe('Measuring...');
    expect(text('detail-peer-jitter')).toBe('5.0ms');
  });
});
