/**
 * SessionStats component tests
 *
 * Covers the latency and jitter figures latency.feature requires the session
 * view to display (REQ-LAT-101, REQ-LAT-103).
 */

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';

import { SessionStats } from './SessionStats';
import type { NetworkStats, DetailedLatency } from '../lib/tauri';

const network: NetworkStats = {
  rtt_ms: 18.42,
  jitter_ms: 0.83,
  packet_loss_percent: 0.1,
  quality: 'good',
  measured_bps: 3_200_000,
  required_bps: 3_100_000,
  bandwidth_status: 'sufficient',
  uptime_seconds: 125,
  packets_sent: 1000,
  packets_received: 998,
  encryption: 'encrypted',
  peer_checked: true,
  bytes_sent: 512_000,
  bytes_received: 511_000,
};

/** Minimal latency payload for tests that only care about the network figures. */
const emptyLatency: DetailedLatency = {
  upstream: [],
  upstream_total_ms: 0,
  downstream: [],
  downstream_total_ms: 0,
  roundtrip_total_ms: 0,
};

describe('SessionStats', () => {
  describe('Latency figures (REQ-LAT-101)', () => {
    // Verifies: REQ-LAT-101
    it('shows the network RTT', () => {
      render(<SessionStats network={network} latency={emptyLatency} />);
      expect(screen.getByText('18.42 ms')).toBeInTheDocument();
    });

    it('shows packet loss', () => {
      render(<SessionStats network={network} latency={emptyLatency} />);
      expect(screen.getByText('0.1 %')).toBeInTheDocument();
    });

    // Verifies: REQ-LAT-130
    it('shows a measuring state rather than 0ms before the first RTT sample', () => {
      // The connection exists (`network` is non-null) but no RTT ping has
      // returned yet, so `rtt_ms` is null - not a misleading 0ms.
      render(
        <SessionStats
          network={{ ...network, rtt_ms: null, quality: null }}
          latency={emptyLatency}
        />
      );
      const rttLabel = screen.getByText('RTT');
      expect(rttLabel.nextElementSibling).toHaveTextContent('Measuring...');
    });

    it('waits rather than showing zeros before the first sample', () => {
      // Before the first poll there is nothing to show, and no misleading 0ms.
      render(<SessionStats network={null} latency={null} />);
      expect(screen.getByText(/Waiting for connection/)).toBeInTheDocument();
    });
  });

  describe('Jitter (REQ-LAT-103)', () => {
    // Verifies: REQ-LAT-103
    it('shows the current jitter value', () => {
      render(<SessionStats network={network} latency={emptyLatency} />);
      expect(screen.getByText('0.83 ms')).toBeInTheDocument();
    });

    it('updates when a new sample arrives', () => {
      const { rerender } = render(<SessionStats network={network} latency={emptyLatency} />);
      expect(screen.getByText('0.83 ms')).toBeInTheDocument();

      rerender(
        <SessionStats
          network={{ ...network, jitter_ms: 12.5 }}
          latency={emptyLatency}
        />
      );
      expect(screen.getByText('12.50 ms')).toBeInTheDocument();
      expect(screen.queryByText('0.83 ms')).not.toBeInTheDocument();
    });
  });

  describe('Total latency breakdown (REQ-LAT-101)', () => {
    const latency: DetailedLatency = {
      upstream: [
        { name: 'Capture', ms: 1.33, info: null },
        { name: 'Network', ms: 9.21, info: null },
      ],
      upstream_total_ms: 10.54,
      downstream: [
        { name: 'Network', ms: 9.21, info: null },
        { name: 'Jitter buffer', ms: 10.67, info: null },
        { name: 'Playback', ms: 1.33, info: null },
      ],
      downstream_total_ms: 21.21,
      roundtrip_total_ms: 31.75,
    };

    // Verifies: REQ-LAT-101
    it('shows the one-way and round-trip totals', () => {
      render(<SessionStats network={network} latency={latency} />);

      // Totals appear in both the breakdown and the summary, hence getAllByText.
      expect(screen.getAllByText('10.54 ms').length).toBeGreaterThan(0);
      expect(screen.getAllByText('21.21 ms').length).toBeGreaterThan(0);
      expect(screen.getAllByText('31.75 ms').length).toBeGreaterThan(0);
    });

    it('shows the jitter buffer contribution to downstream latency', () => {
      render(<SessionStats network={network} latency={latency} />);
      expect(screen.getAllByText('10.67 ms').length).toBeGreaterThan(0);
    });
  });
});
