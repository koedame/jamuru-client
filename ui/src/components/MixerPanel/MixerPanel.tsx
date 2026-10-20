/**
 * MixerPanel - Root component for the audio mixing console
 * jamuru brand (ui.pen Screens/Main mixerPanel): the local ("input") channel
 * and the remote ("output") channels grouped either side of a vertical divider.
 */

import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { ChannelStrip } from "./ChannelStrip";
import "./MixerPanel.css";

export interface Channel {
  /** Unique identifier */
  id: string;
  /** Display name */
  name: string;
  /** Channel type */
  type: "local" | "remote";
  /** Sample rate in Hz */
  sampleRate: number;
  /** Number of channels (1 or 2) */
  channelCount: number;
  /** Left channel level (0-100) */
  levelL: number;
  /** Right channel level (0-100) */
  levelR: number;
  /** Volume (0-100) */
  volume: number;
  /** Pan (-100 to 100) */
  pan: number;
  /** Whether the channel is muted */
  isMuted: boolean;
  /** Whether the user hears their own input directly (local channel only) */
  isMonitoring?: boolean;
}

export interface MixerPanelProps {
  /** Array of channels to display */
  channels: Channel[];
  /** Callback when a channel's volume changes */
  onChannelVolumeChange?: (channelId: string, volume: number) => void;
  /** Callback when a channel's pan changes */
  onChannelPanChange?: (channelId: string, pan: number) => void;
  /** Callback when a channel's mute is toggled */
  onChannelMuteToggle?: (channelId: string) => void;
  /** Callback when the local channel's monitoring is toggled */
  onChannelMonitorToggle?: (channelId: string) => void;
}

export function MixerPanel({
  channels,
  onChannelVolumeChange,
  onChannelPanChange,
  onChannelMuteToggle,
  onChannelMonitorToggle,
}: MixerPanelProps) {
  const { t } = useTranslation();

  // Single pass instead of filtering the array twice.
  const [localChannels, remoteChannels] = useMemo(() => {
    const local: Channel[] = [];
    const remote: Channel[] = [];
    for (const c of channels) {
      (c.type === "local" ? local : remote).push(c);
    }
    return [local, remote];
  }, [channels]);

  const renderStrip = (channel: Channel) => (
    <ChannelStrip
      key={channel.id}
      id={channel.id}
      name={channel.name}
      type={channel.type}
      sampleRate={channel.sampleRate}
      channelCount={channel.channelCount}
      levelL={channel.levelL}
      levelR={channel.levelR}
      volume={channel.volume}
      pan={channel.pan}
      isMuted={channel.isMuted}
      isMonitoring={channel.isMonitoring}
      onVolumeChange={
        onChannelVolumeChange ? (volume) => onChannelVolumeChange(channel.id, volume) : undefined
      }
      onPanChange={
        onChannelPanChange ? (pan) => onChannelPanChange(channel.id, pan) : undefined
      }
      onMuteToggle={
        onChannelMuteToggle ? () => onChannelMuteToggle(channel.id) : undefined
      }
      onMonitorToggle={
        onChannelMonitorToggle ? () => onChannelMonitorToggle(channel.id) : undefined
      }
    />
  );

  return (
    <div className="mixer-panel" data-testid="mixer-panel" role="region" aria-label={t("mixer.title")}>
      <span className="mixer-panel__title">{t("mixer.title")}</span>

      <div className="mixer-panel__channels">
        <div className="mixer-panel__section">{localChannels.map(renderStrip)}</div>
        {remoteChannels.length > 0 && (
          <>
            <div className="mixer-panel__divider" />
            <div className="mixer-panel__section">{remoteChannels.map(renderStrip)}</div>
          </>
        )}
      </div>
    </div>
  );
}

export default MixerPanel;
