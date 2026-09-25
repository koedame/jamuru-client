/**
 * AudioQualityBadge - Displays audio quality info
 *
 * Format: "{sampleRate}kHz/{channels}ch"
 * Examples: "48kHz/2ch", "44.1kHz/1ch"
 */

import "./AudioQualityBadge.css";

export interface AudioQualityBadgeProps {
  /** Sample rate in Hz (e.g., 48000, 44100) */
  sampleRate: number;
  /** Number of channels (1 = mono, 2 = stereo) */
  channels: number;
}

function formatSampleRate(hz: number): string {
  const khz = hz / 1000;
  // Use one decimal for non-integer kHz (e.g., 44.1)
  if (khz % 1 !== 0) {
    return khz.toFixed(1);
  }
  return khz.toString();
}

export function AudioQualityBadge({
  sampleRate,
  channels,
}: AudioQualityBadgeProps) {
  const displayRate = formatSampleRate(sampleRate);

  return (
    // data-sample-rate carries the rate in Hz next to the rounded kHz text.
    <div className="audio-quality-badge" data-testid="audio-quality-badge" data-sample-rate={sampleRate}>
      <span className="audio-quality-badge__rate">{displayRate}kHz</span>
      <span className="audio-quality-badge__separator">/</span>
      <span className="audio-quality-badge__channels">{channels}ch</span>
    </div>
  );
}

export default AudioQualityBadge;
