import { describe, expect, it } from "vitest";
import {
  linkTrouble,
  localBuffers,
  measuredLatency,
  overallLink,
  slowestPeerId,
  worstBandwidthLink,
} from "./links";
import type { BandwidthStatus, ConnectionQualityBand, PeerLinkStatus } from "./tauri";

/**
 * A link in `state`. With `measured`, one that is up: it has received
 * `packets` of its peer (some, unless said) and has `rtt` and `quality` as
 * measured, or not yet.
 */
function link(
  peerId: string,
  state: string | null,
  measured?: {
    rtt?: number;
    quality?: ConnectionQualityBand;
    bandwidth?: BandwidthStatus;
    packets?: number;
  }
): PeerLinkStatus {
  return {
    peer_id: peerId,
    connection_state: state,
    connection_error: null,
    network: measured
      ? {
          rtt_ms: measured.rtt ?? null,
          jitter_ms: 0,
          packet_loss_percent: 0,
          quality: measured.quality ?? null,
          measured_bps: 0,
          required_bps: 0,
          bandwidth_status: measured.bandwidth ?? null,
          uptime_seconds: 0,
          packets_sent: 0,
          packets_received: measured.packets ?? 10,
          encryption: "encrypted",
          peer_checked: true,
          bytes_sent: 0,
          bytes_received: 0,
        }
      : null,
    // The backend gives a breakdown as soon as the link is up, measured or not
    latency: measured
      ? {
          upstream: [],
          upstream_total_ms: (measured.rtt ?? 0) / 2,
          downstream: [],
          downstream_total_ms: (measured.rtt ?? 0) / 2,
          roundtrip_total_ms: measured.rtt ?? 0,
        }
      : null,
    peer_audio: null,
    level: 0,
  };
}

describe("linkTrouble", () => {
  it("a participant with no link yet is one the app is connecting to", () => {
    expect(linkTrouble(undefined)).toBe("connecting");
  });

  it("a link still checking its candidates is connecting", () => {
    expect(linkTrouble(link("a", "checkingconnectivity"))).toBe("connecting");
  });

  it("a link that is up and has received packets of its peer has no trouble", () => {
    expect(linkTrouble(link("a", "connected", { rtt: 12 }))).toBeNull();
    expect(linkTrouble(link("a", "connected", {}))).toBeNull();
  });

  // Verifies: REQ-CON-134
  it("a link that is up but has received nothing of its peer is still connecting", () => {
    expect(linkTrouble(link("a", "connected", { packets: 0 }))).toBe("connecting");
  });

  it("a link that went silent is unstable and one that gave up is an error", () => {
    expect(linkTrouble(link("a", "reconnecting", { rtt: 12 }))).toBe("unstable");
    expect(linkTrouble(link("a", "failed", { rtt: 12 }))).toBe("error");
  });
});

describe("measuredLatency", () => {
  it("a link that hears its peer and has measured a round trip has its latency", () => {
    expect(measuredLatency(link("a", "connected", { rtt: 12 }))?.roundtrip_total_ms).toBe(12);
  });

  // Verifies: REQ-LAT-130
  it("a link with no round trip measured yet has no latency to show", () => {
    expect(measuredLatency(link("a", "connected", {}))).toBeNull();
  });

  it("a link that is not up, or no link at all, has no latency to show", () => {
    expect(measuredLatency(link("a", "reconnecting", { rtt: 12 }))).toBeNull();
    expect(measuredLatency(undefined)).toBeNull();
  });
});

describe("overallLink", () => {
  it("with no link there is no trouble and nothing measured", () => {
    expect(overallLink([])).toEqual({ trouble: null, quality: undefined });
  });

  // Verifies: REQ-LAT-102
  it("when every link is up, shows the worst quality among them", () => {
    const overall = overallLink([
      link("a", "connected", { rtt: 8, quality: "good" }),
      link("b", "connected", { rtt: 42, quality: "fair" }),
      link("c", "connected", { rtt: 15, quality: "good" }),
    ]);

    expect(overall).toEqual({ trouble: null, quality: "fair" });
  });

  it("when one link is reconnecting while the others are up, the room is unstable", () => {
    const overall = overallLink([
      link("a", "connected", { rtt: 8, quality: "good" }),
      link("b", "reconnecting", { rtt: 20, quality: "good" }),
    ]);

    expect(overall.trouble).toBe("unstable");
  });

  it("when one link gave up, the room shows the error even while another reconnects", () => {
    const overall = overallLink([
      link("a", "failed", { rtt: 8, quality: "poor" }),
      link("b", "reconnecting", { rtt: 20, quality: "good" }),
      link("c", "connected", { rtt: 20, quality: "good" }),
    ]);

    expect(overall.trouble).toBe("error");
  });

  it("a link still connecting is not a trouble of the room", () => {
    expect(overallLink([link("a", "checkingconnectivity")]).trouble).toBeNull();
  });
});

describe("slowestPeerId", () => {
  // Verifies: REQ-LAT-102
  it("with several participants measured, names the one with the longest round trip", () => {
    const slowest = slowestPeerId([
      link("a", "connected", { rtt: 8 }),
      link("b", "connected", { rtt: 42 }),
      link("c", "connected", { rtt: 15 }),
    ]);

    expect(slowest).toBe("b");
  });

  it("with one participant there is nobody to compare with", () => {
    expect(slowestPeerId([link("a", "connected", { rtt: 8 })])).toBeNull();
  });

  it("a participant whose link is not up is not compared", () => {
    const slowest = slowestPeerId([
      link("a", "connected", { rtt: 8 }),
      link("b", "failed", { rtt: 900 }),
      link("c", "connected", { rtt: 15 }),
    ]);

    expect(slowest).toBe("c");
  });
});

describe("worstBandwidthLink", () => {
  it("with no reading on any link there is none", () => {
    expect(worstBandwidthLink([link("a", "connected", { rtt: 8 })])).toBeNull();
  });

  it("names the link whose reading is the worst, no signal being worse than insufficient", () => {
    const worst = worstBandwidthLink([
      link("a", "connected", { rtt: 8, bandwidth: "sufficient" }),
      link("b", "connected", { rtt: 8, bandwidth: "no_signal" }),
      link("c", "connected", { rtt: 8, bandwidth: "insufficient" }),
    ]);

    expect(worst?.peer_id).toBe("b");
  });
});

describe("localBuffers", () => {
  // Verifies: REQ-LAT-122
  it("when the breakdown has capture, playback and jitter buffers, gives this machine's input, output and jitter delay", () => {
    const latency = {
      upstream: [
        { name: "Capture buffer", ms: 0.67, info: null },
        { name: "Network", ms: 5, info: null },
      ],
      upstream_total_ms: 5.67,
      downstream: [
        { name: "Network", ms: 5, info: null },
        { name: "Jitter buffer", ms: 2, info: null },
        { name: "Playback buffer", ms: 1.33, info: null },
      ],
      downstream_total_ms: 8.33,
      roundtrip_total_ms: 14,
    };

    expect(localBuffers(latency)).toEqual({ input: 0.67, output: 1.33, jitter: 2 });
  });

  it("when the breakdown lacks a buffer, or there is none, gives nothing rather than a guess", () => {
    expect(localBuffers(null)).toBeNull();
    expect(
      localBuffers({
        upstream: [],
        upstream_total_ms: 0,
        downstream: [],
        downstream_total_ms: 0,
        roundtrip_total_ms: 0,
      })
    ).toBeNull();
  });
});
