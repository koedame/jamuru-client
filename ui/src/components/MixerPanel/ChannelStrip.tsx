/**
 * ChannelStrip - Single channel in the mixer panel
 * jamuru brand: quality badge, pan slider,
 * fader + meter with dB read-outs, name, and a mute button. The local strip
 * also carries the monitor button (hear yourself without the network delay).
 */

import { useTranslation } from "react-i18next";
import { PanSlider } from "./PanSlider";
import { StereoMeterView } from "./StereoMeter";
import { StereoFader } from "./StereoFader";
import { AudioQualityBadge } from "./AudioQualityBadge";
import { DropoutNotice } from "../DropoutNotice";
import type { PeerDropout } from "../../lib/tauri";
import { usePeakHold } from "./usePeakHold";
import { levelToDb } from "./mixerScale";
import { HeadphonesIcon, MicIcon, MicOffIcon, VolumeIcon, VolumeOffIcon } from "../../lib/icons";
import "./ChannelStrip.css";

export type ChannelType = "local" | "remote";

export interface ChannelStripProps {
  /** Channel identifier */
  id: string;
  /** Display name */
  name: string;
  /** Channel type */
  type: ChannelType;
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
  /** Set while nothing arrives from the participant and they have not left (remote only) */
  dropout?: PeerDropout | null;
  /** Went quiet twice or more in the last quarter of an hour (remote only) */
  unstable?: boolean;
  /** Callback when volume changes */
  onVolumeChange?: (volume: number) => void;
  /** Callback when pan changes */
  onPanChange?: (pan: number) => void;
  /** Callback when mute is toggled */
  onMuteToggle?: () => void;
  /** Whether the user hears their own input directly (local channel only) */
  isMonitoring?: boolean;
  /** Callback when monitoring is toggled. The button is shown on the local
   *  channel only, and only when this is given. */
  onMonitorToggle?: () => void;
}

// Convert volume (0-100) to dB for fader display (80 = 0dB unity gain)
function volumeToDb(volume: number): string {
  if (volume === 0) return "-∞";
  const db = 20 * Math.log10(volume / 80);
  if (db >= 0) return `+${db.toFixed(1)}`;
  return db.toFixed(1);
}

/** The meter and its peak read-out. The peak falls on a timer while it is up;
 *  holding it here draws only this column on each tick, not the whole strip. */
function ChannelMeter({
  levelL,
  levelR,
  isMuted,
}: {
  levelL: number;
  levelR: number;
  isMuted: boolean;
}) {
  // Use peak hold for stable dB display
  const peaks = usePeakHold(levelL, levelR, { holdTime: 500, releaseTime: 1500 });
  const maxPeak = isMuted ? 0 : Math.max(peaks.peakL, peaks.peakR);

  return (
    <div className="channel-strip__col">
      {/* The meter's numeric read-out. data-peak carries the raw 0-100
          level so a test can assert on the signal without parsing a dB
          string (which is localised and rounded). */}
      <span className="channel-strip__value" data-testid="channel-peak" data-peak={maxPeak}>
        {levelToDb(maxPeak)}
      </span>
      <StereoMeterView
        levelL={levelL}
        levelR={levelR}
        peakL={peaks.peakL}
        peakR={peaks.peakR}
        isMuted={isMuted}
      />
    </div>
  );
}

export function ChannelStrip({
  id,
  name,
  type,
  sampleRate,
  channelCount,
  levelL,
  levelR,
  volume,
  pan,
  isMuted,
  onVolumeChange,
  onPanChange,
  onMuteToggle,
  isMonitoring = false,
  dropout = null,
  unstable = false,
  onMonitorToggle,
}: ChannelStripProps) {
  const { t } = useTranslation();

  const displayName = type === "local" ? t("mixer.self") : name;

  // Fader setting in dB for display
  const faderDb = volumeToDb(volume);

  // Mic icon for local (mine) channels, speaker icon for remote channels
  const muteIcon =
    type === "local"
      ? isMuted
        ? <MicOffIcon size={18} />
        : <MicIcon size={18} />
      : isMuted
        ? <VolumeOffIcon size={18} />
        : <VolumeIcon size={18} />;

  // data-channel-type tells the user's own strip from a peer's, which is what
  // distinguishes "my microphone" from "what I receive".
  return (
    <div
      className={`channel-strip channel-strip--${type}`}
      data-channel-id={id}
      data-testid="channel-strip"
      data-channel-type={type}
    >
      <AudioQualityBadge sampleRate={sampleRate} channels={channelCount} />

      <PanSlider value={pan} onChange={onPanChange} label={t("mixer.channel.panLabel", { name: displayName })} />

      <div className="channel-strip__meters">
        <div className="channel-strip__col">
          <span className="channel-strip__value">{faderDb}</span>
          <StereoFader
            volume={volume}
            onChange={onVolumeChange}
            label={t("mixer.channel.volumeLabel", { name: displayName })}
          />
        </div>
        <ChannelMeter levelL={levelL} levelR={levelR} isMuted={isMuted} />
      </div>

      <DropoutNotice dropout={dropout} unstable={unstable} variant="strip" />

      <div className="channel-strip__name">{displayName}</div>

      <div className="channel-strip__buttons">
        <button
          type="button"
          className={`channel-strip__mute ${isMuted ? "channel-strip__mute--muted" : "channel-strip__mute--active"}`}
          onClick={onMuteToggle}
          data-testid="channel-mute"
          aria-label={t(isMuted ? "mixer.channel.unmute" : "mixer.channel.mute")}
          aria-pressed={isMuted}
        >
          {muteIcon}
        </button>

        {type === "local" && onMonitorToggle && (
          <button
            type="button"
            className={`channel-strip__monitor ${isMonitoring ? "channel-strip__monitor--on" : ""}`}
            onClick={onMonitorToggle}
            data-testid="channel-monitor"
            aria-label={t("mixer.channel.monitor")}
            title={t("mixer.channel.monitorHint")}
            aria-pressed={isMonitoring}
          >
            <HeadphonesIcon size={18} />
          </button>
        )}
      </div>
    </div>
  );
}

export default ChannelStrip;
