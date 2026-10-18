/**
 * What the screen shows of the audio links: one to each participant.
 *
 * Pure, so the rules - which link the footer stands for, whose latency is
 * marked, what a row says while a link is not up - are tested without a
 * backend.
 */
import type { ConnectionQualityBand, DetailedLatency, PeerLinkStatus } from "./tauri";

/** What a participant's row says in place of a latency while the link to them is not up */
export type LinkTrouble = "connecting" | "unstable" | "error";

/**
 * How the link to a participant is doing, or null when they are heard.
 *
 * A participant there is no link to yet has not published an address, or has
 * only just done so: the link is on its way. So is one that is up but has
 * received nothing of theirs: a link comes up on an address before anything
 * has answered there.
 */
export function linkTrouble(link: PeerLinkStatus | undefined): LinkTrouble | null {
  switch (link?.connection_state) {
    case "failed":
      return "error";
    case "reconnecting":
      return "unstable";
    case "connected":
      return (link.network?.packets_received ?? 0) > 0 ? null : "connecting";
    default:
      return "connecting";
  }
}

/**
 * The latency to show of a link: its breakdown once the link hears its peer
 * and has measured a round trip. A breakdown with no measurement behind it is
 * not a latency (REQ-LAT-130).
 */
export function measuredLatency(link: PeerLinkStatus | undefined): DetailedLatency | null {
  if (!link || linkTrouble(link) !== null || link.network?.rtt_ms == null) return null;
  return link.latency;
}

/** This machine's side of a link, as the latency breakdown has it (REQ-LAT-122) */
export interface LocalBuffers {
  /** Capture (microphone side) delay in ms */
  input: number;
  /** Playback (headphone side) delay in ms */
  output: number;
  /** The delay held against jitter, in ms */
  jitter: number;
}

/**
 * This machine's input, output and jitter buffer delays, taken from the
 * breakdown rather than recomputed. They are the machine's own, so any link's
 * breakdown has them. Null while the breakdown has no such components.
 */
export function localBuffers(latency: DetailedLatency | null | undefined): LocalBuffers | null {
  if (!latency) return null;
  const capture = latency.upstream.find((c) => c.name.includes("Capture"));
  const playback = latency.downstream.find((c) => c.name.includes("Playback"));
  const jitter = latency.downstream.find((c) => c.name.includes("Jitter"));
  if (!capture || !playback || !jitter) return null;
  return { input: capture.ms, output: playback.ms, jitter: jitter.ms };
}

const QUALITY_ORDER: ConnectionQualityBand[] = ["good", "fair", "poor"];

/** The links as one: what the footer shows of the room */
export interface OverallLink {
  /** "error" when a link gave up, "unstable" when one is reconnecting, else null */
  trouble: "error" | "unstable" | null;
  /** The worst quality band among the links that have one */
  quality: ConnectionQualityBand | undefined;
}

/**
 * The room is doing as well as its worst link: a trouble with one participant
 * is one the person has to see.
 */
export function overallLink(links: PeerLinkStatus[]): OverallLink {
  const states = links.map((link) => link.connection_state);
  const trouble = states.includes("failed")
    ? "error"
    : states.includes("reconnecting")
      ? "unstable"
      : null;
  let quality: ConnectionQualityBand | undefined;
  for (const link of links) {
    const band = link.network?.quality;
    if (band && (!quality || QUALITY_ORDER.indexOf(band) > QUALITY_ORDER.indexOf(quality))) {
      quality = band;
    }
  }
  return { trouble, quality };
}

/**
 * The participant whose audio takes longest to go there and back, when there
 * are at least two to compare (REQ-LAT-102). Null with one link or none.
 */
export function slowestPeerId(links: PeerLinkStatus[]): string | null {
  const measured = links
    .map((link) => ({ peerId: link.peer_id, latency: measuredLatency(link) }))
    .filter((link): link is { peerId: string; latency: DetailedLatency } => link.latency !== null);
  if (measured.length < 2) return null;
  let slowest = measured[0];
  for (const link of measured) {
    if (link.latency.roundtrip_total_ms > slowest.latency.roundtrip_total_ms) {
      slowest = link;
    }
  }
  return slowest.peerId;
}

const BANDWIDTH_ORDER = ["sufficient", "marginal", "insufficient", "no_signal"];

/** The link whose bandwidth reading is the worst, or null when none has a reading */
export function worstBandwidthLink(links: PeerLinkStatus[]): PeerLinkStatus | null {
  let worst: PeerLinkStatus | null = null;
  for (const link of links) {
    const status = link.network?.bandwidth_status;
    if (!status) continue;
    const worstStatus = worst?.network?.bandwidth_status ?? "";
    if (BANDWIDTH_ORDER.indexOf(status) > BANDWIDTH_ORDER.indexOf(worstStatus)) {
      worst = link;
    }
  }
  return worst;
}
