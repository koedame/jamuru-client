/**
 * SessionStats - Detailed session statistics display
 *
 * Shows network stats and latency breakdown similar to CLI output.
 */

import { useTranslation } from "react-i18next";
import type { NetworkStats, DetailedLatency, PeerAudioInfo } from "../lib/tauri";
import "./SessionStats.css";

export interface SessionStatsProps {
  network: NetworkStats | null;
  latency: DetailedLatency | null;
  /** Underrun rate per second */
  underrunRate?: number;
  /** Peer's audio configuration */
  peerAudio?: PeerAudioInfo | null;
  /** Local sample rate for comparison */
  localSampleRate?: number;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function SessionStats({ network, latency, underrunRate = 0, peerAudio, localSampleRate = 48000 }: SessionStatsProps) {
  const { t } = useTranslation();

  if (!network || !latency) {
    return (
      <div className="session-stats session-stats--empty">
        <p>{t("sessionStats.waiting")}</p>
      </div>
    );
  }

  return (
    <div className="session-stats">
      {/* Network Section */}
      <section className="session-stats__section">
        <h3 className="session-stats__section-title">{t("sessionStats.network")}</h3>
        <div className="session-stats__grid">
          <div className="session-stats__item">
            <span className="session-stats__label">{t("sessionStats.rtt")}</span>
            <span className="session-stats__value">
              {network.rtt_ms !== null ? `${network.rtt_ms.toFixed(2)} ms` : t("sessionStats.measuring")}
            </span>
          </div>
          <div className="session-stats__item">
            <span className="session-stats__label">{t("sessionStats.jitter")}</span>
            <span className="session-stats__value">{network.jitter_ms.toFixed(2)} ms</span>
          </div>
          <div className="session-stats__item">
            <span className="session-stats__label">{t("sessionStats.packetLoss")}</span>
            <span className="session-stats__value">{network.packet_loss_percent.toFixed(1)} %</span>
          </div>
          <div className="session-stats__item">
            <span className="session-stats__label">{t("sessionStats.underruns")}</span>
            <span className={`session-stats__value ${underrunRate > 0.5 ? "session-stats__value--warning" : ""}`}>
              {underrunRate.toFixed(1)}/s
            </span>
          </div>
        </div>
      </section>

      {/* Peer Audio Section */}
      {peerAudio && (
        <section className="session-stats__section">
          <h3 className="session-stats__section-title">{t("sessionStats.peerAudio")}</h3>
          <div className="session-stats__grid">
            <div className="session-stats__item">
              <span className="session-stats__label">{t("sessionStats.sampleRate")}</span>
              <span className={`session-stats__value ${peerAudio.needs_resampling ? "session-stats__value--warning" : ""}`}>
                {(peerAudio.sample_rate / 1000).toFixed(1)} kHz
                {peerAudio.needs_resampling && " ⚠"}
              </span>
            </div>
            <div className="session-stats__item">
              <span className="session-stats__label">{t("sessionStats.frameSize")}</span>
              <span className="session-stats__value">{t("sessionStats.frameSizeValue", { size: peerAudio.frame_size })}</span>
            </div>
            <div className="session-stats__item">
              <span className="session-stats__label">{t("sessionStats.codec")}</span>
              <span className="session-stats__value">{peerAudio.codec.toUpperCase()}</span>
            </div>
            <div className="session-stats__item">
              <span className="session-stats__label">{t("sessionStats.localRate")}</span>
              <span className="session-stats__value">{(localSampleRate / 1000).toFixed(1)} kHz</span>
            </div>
          </div>
          {peerAudio.needs_resampling && (
            <div className="session-stats__resampling-warning">
              <span className="session-stats__warning-icon">⚠</span>
              <span className="session-stats__warning-text">
                {t("sessionStats.resampling", {
                  from: (peerAudio.sample_rate / 1000).toFixed(1),
                  to: (localSampleRate / 1000).toFixed(1),
                })}
              </span>
            </div>
          )}
        </section>
      )}

      {/* Latency Breakdown Section */}
      <section className="session-stats__section">
        <h3 className="session-stats__section-title">{t("sessionStats.latencyBreakdown")}</h3>

        {/* Upstream */}
        <div className="session-stats__latency-group">
          <h4 className="session-stats__latency-title">
            {t("sessionStats.upstream")}
          </h4>
          <div className="session-stats__latency-items">
            {latency.upstream.map((component, index) => (
              <div key={index} className="session-stats__latency-item">
                <span className="session-stats__latency-name">{component.name}</span>
                <span className="session-stats__latency-value">
                  {component.ms.toFixed(2)} ms
                  {component.info && (
                    <span className="session-stats__latency-info">({component.info})</span>
                  )}
                </span>
              </div>
            ))}
            <div className="session-stats__latency-item session-stats__latency-item--total">
              <span className="session-stats__latency-name">{t("sessionStats.total")}</span>
              <span className="session-stats__latency-value">{latency.upstream_total_ms.toFixed(2)} ms</span>
            </div>
          </div>
        </div>

        {/* Downstream */}
        <div className="session-stats__latency-group">
          <h4 className="session-stats__latency-title">
            {t("sessionStats.downstream")}
          </h4>
          <div className="session-stats__latency-items">
            {latency.downstream.map((component, index) => (
              <div key={index} className="session-stats__latency-item">
                <span className="session-stats__latency-name">{component.name}</span>
                <span className="session-stats__latency-value">
                  {component.ms.toFixed(2)} ms
                  {component.info && (
                    <span className="session-stats__latency-info">({component.info})</span>
                  )}
                </span>
              </div>
            ))}
            <div className="session-stats__latency-item session-stats__latency-item--total">
              <span className="session-stats__latency-name">{t("sessionStats.total")}</span>
              <span className="session-stats__latency-value">{latency.downstream_total_ms.toFixed(2)} ms</span>
            </div>
          </div>
        </div>
      </section>

      {/* Summary */}
      <section className="session-stats__section">
        <h3 className="session-stats__section-title">{t("sessionStats.summary")}</h3>
        <div className="session-stats__summary">
          <div className="session-stats__summary-item">
            <span className="session-stats__label">{t("sessionStats.upstreamShort")}</span>
            <span className="session-stats__value session-stats__value--highlight">
              {latency.upstream_total_ms.toFixed(2)} ms
            </span>
          </div>
          <div className="session-stats__summary-item">
            <span className="session-stats__label">{t("sessionStats.downstreamShort")}</span>
            <span className="session-stats__value session-stats__value--highlight">
              {latency.downstream_total_ms.toFixed(2)} ms
            </span>
          </div>
          <div className="session-stats__summary-item">
            <span className="session-stats__label">{t("sessionStats.roundTrip")}</span>
            <span className="session-stats__value session-stats__value--highlight">
              {latency.roundtrip_total_ms.toFixed(2)} ms
            </span>
          </div>
        </div>
      </section>

      {/* Packets */}
      <section className="session-stats__section">
        <h3 className="session-stats__section-title">{t("sessionStats.packets")}</h3>
        <div className="session-stats__grid">
          <div className="session-stats__item">
            <span className="session-stats__label">{t("sessionStats.sent")}</span>
            <span className="session-stats__value">{network.packets_sent.toLocaleString()}</span>
          </div>
          <div className="session-stats__item">
            <span className="session-stats__label">{t("sessionStats.received")}</span>
            <span className="session-stats__value">{network.packets_received.toLocaleString()}</span>
          </div>
          <div className="session-stats__item">
            <span className="session-stats__label">{t("sessionStats.bytesSent")}</span>
            <span className="session-stats__value">{formatBytes(Number(network.bytes_sent))}</span>
          </div>
          <div className="session-stats__item">
            <span className="session-stats__label">{t("sessionStats.bytesReceived")}</span>
            <span className="session-stats__value">{formatBytes(Number(network.bytes_received))}</span>
          </div>
        </div>
      </section>
    </div>
  );
}

export default SessionStats;
