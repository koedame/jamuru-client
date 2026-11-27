/**
 * ConnectionIndicator component
 *
 * Displays real-time connection status with optional latency value.
 */

import { useTranslation } from 'react-i18next';
import {
  DisconnectedIcon,
  ConnectingIcon,
  ConnectedIcon,
  UnstableIcon,
  ErrorIcon,
} from './icons';
import './ConnectionIndicator.css';

export type ConnectionStatus =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'unstable'
  | 'error';

/**
 * Connection quality band, classified by the core library
 *
 * Passed in rather than computed here: the RTT and loss thresholds live in
 * `src/network/quality.rs` so that the UI and the specification cannot drift.
 */
export type ConnectionQuality = 'good' | 'fair' | 'poor';

export interface ConnectionIndicatorProps {
  /** Connection status */
  status: ConnectionStatus;

  /**
   * Connection quality band. Drives the indicator colour while connected.
   *
   * Omit when there is no measurement yet; the status colour is used instead.
   */
  quality?: ConnectionQuality;

  /** Audio device input latency in milliseconds */
  inputLatencyMs?: number;

  /** Audio device output latency in milliseconds */
  outputLatencyMs?: number;

  /** RTT (round-trip time) in milliseconds (deprecated, use upstreamLatencyMs/downstreamLatencyMs) */
  latencyMs?: number;

  /** Upstream latency (self -> peer) in milliseconds */
  upstreamLatencyMs?: number;

  /** Downstream latency (peer -> self) in milliseconds */
  downstreamLatencyMs?: number;

  /** Whether to show latency value */
  showLatency?: boolean;

  /** Component size */
  size?: 'sm' | 'md' | 'lg';

  /** Click handler (e.g., navigate to connection details) */
  onClick?: () => void;
}

const iconSizes = {
  sm: 12,
  md: 16,
  lg: 20,
};

function getIcon(status: ConnectionStatus, size: number) {
  const iconProps = { size, className: 'connection-indicator__icon' };

  switch (status) {
    case 'disconnected':
      return <DisconnectedIcon {...iconProps} />;
    case 'connecting':
      return <ConnectingIcon {...iconProps} />;
    case 'connected':
      return <ConnectedIcon {...iconProps} />;
    case 'unstable':
      return <UnstableIcon {...iconProps} />;
    case 'error':
      return <ErrorIcon {...iconProps} />;
  }
}

export function ConnectionIndicator({
  status,
  quality,
  inputLatencyMs,
  outputLatencyMs,
  latencyMs,
  upstreamLatencyMs,
  downstreamLatencyMs,
  showLatency = true,
  size = 'md',
  onClick,
}: ConnectionIndicatorProps) {
  const { t } = useTranslation();

  const statusText = t(`status.${status}`);

  // Build latency text: prefer upstream/downstream if available
  let latencyText: string | null = null;
  if (showLatency) {
    if (upstreamLatencyMs !== undefined && downstreamLatencyMs !== undefined) {
      // Show both directions: "↑12ms ↓12ms"
      latencyText = `↑${Math.round(upstreamLatencyMs)}ms ↓${Math.round(downstreamLatencyMs)}ms`;
    } else if (latencyMs !== undefined) {
      // Fallback to single RTT value
      latencyText = t('status.latency', { ms: Math.round(latencyMs) });
    }
  }

  // Device input/output latency, shown separately from network latency so the
  // user can tell which half to act on.
  let deviceLatencyText: string | null = null;
  if (showLatency && inputLatencyMs !== undefined && outputLatencyMs !== undefined) {
    deviceLatencyText = t('status.deviceLatency', {
      input: Math.round(inputLatencyMs),
      output: Math.round(outputLatencyMs),
      defaultValue: `in ${Math.round(inputLatencyMs)}ms / out ${Math.round(outputLatencyMs)}ms`,
    });
  }

  // Build aria-label for screen readers
  const ariaLabelParts = [statusText];
  if (quality) {
    ariaLabelParts.push(t(`quality.${quality}`, { defaultValue: quality }));
  }
  if (latencyText) {
    ariaLabelParts.push(latencyText);
  }
  if (deviceLatencyText) {
    ariaLabelParts.push(deviceLatencyText);
  }
  const ariaLabel =
    ariaLabelParts.length > 1
      ? `${ariaLabelParts[0]} (${ariaLabelParts.slice(1).join(', ')})`
      : statusText;

  const classNames = [
    'connection-indicator',
    `connection-indicator--${status}`,
    `connection-indicator--${size}`,
    // Quality drives the colour while connected; the class is only emitted when
    // a measurement exists, so an unmeasured link keeps the status colour.
    quality ? `connection-indicator--quality-${quality}` : '',
    onClick ? 'connection-indicator--clickable' : '',
  ]
    .filter(Boolean)
    .join(' ');

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (onClick && (e.key === 'Enter' || e.key === ' ')) {
      e.preventDefault();
      onClick();
    }
  };

  return (
    <div
      role="status"
      aria-live="polite"
      aria-label={ariaLabel}
      className={classNames}
      onClick={onClick}
      onKeyDown={handleKeyDown}
      tabIndex={onClick ? 0 : undefined}
    >
      <span aria-hidden="true">{getIcon(status, iconSizes[size])}</span>
      <span className="connection-indicator__text">{statusText}</span>
      {latencyText && (
        <span className="connection-indicator__latency">({latencyText})</span>
      )}
      {deviceLatencyText && (
        <span className="connection-indicator__device-latency">
          {deviceLatencyText}
        </span>
      )}
    </div>
  );
}

export default ConnectionIndicator;
