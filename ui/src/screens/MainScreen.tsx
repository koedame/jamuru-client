/**
 * Main Screen
 *
 * Entry point for session creation and joining.
 * Displays connection status and provides room management UI.
 *
 * The session - connecting, the room, who is in it, the audio link, getting
 * the connection back - is the backend's (ADR-044 §6). This draws the
 * snapshot it reads with `session_get` and hears with `session:changed`, and
 * acts with one command per operation. What stays here is what only the
 * screen has: dialogs, the code being typed, the mixer's faders.
 *
 * A helper's window draws this same screen from the helped app's state, through
 * the relay (`helper`, ADR-044 §5). The screen is the same code; it just does not
 * offer what the helped app would refuse a helper - speaking for them, leaving
 * the room - and does not ask for what is the helped person's own: their history
 * of rooms, the server they use.
 */
import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { ConnectionPanel, type ConnectionState, type ConnectionErrorKind, type ConnectionHistoryEntry as ConnectionPanelHistoryEntry } from "../components/ConnectionPanel";
import { MixerPanel, MasterSection, type Channel } from "../components/MixerPanel";
import { ChatPanelAdapter } from "../components/ChatPanel";
import { ConnectionIndicator, type ConnectionStatus } from "../components/ConnectionIndicator";
import { Toast } from "../components/Toast";
import { LeaveDialog } from "../components/LeaveDialog";
import { useSettingsHelp } from "../components/SettingsHelp";
import { formatErrorForDisplay } from "../lib/errorMessages";
import { registerInviteLinkHandler } from "../lib/deepLink";
import { listenEvent } from "../lib/backend";
import { useWindowEvent } from "../hooks/useWindowEvents";
import {
  sessionGet,
  sessionConnect,
  sessionCreate,
  sessionJoin,
  sessionLeave,
  sessionReconnect,
  SESSION_CHANGED,
  SESSION_SETTINGS_HELP,
  type SessionSnapshot,
  AUDIO_SETTINGS_CHANGED,
  type AudioSettings,
  streamingReconnect,
  streamingStatus,
  streamingSetMute,
  streamingSetMonitoring,
  mixerGet,
  mixerSetLocalVolume,
  mixerSetLocalPan,
  mixerSetPeerVolume,
  mixerSetPeerPan,
  mixerSetPeerMuted,
  MIXER_CHANGED,
  type MixerPeerStrip,
  type MixerSnapshot,
  configGetConnectionHistory,
  configRemoveConnectionHistory,
  configGetSampleRate,
  configGetTransmitChannels,
  configGetEffectiveServerUrl,
  windowResizeMain,
  type HelpEvent,
  type NetworkStats,
  type DetailedLatency,
  type ConnectionHistoryEntry,
  type PeerAudioInfo,
} from "../lib/tauri";
import { JOIN_WINDOW_SIZE, MIXER_WINDOW_SIZE, JOIN_MIN_SIZE, MIXER_MIN_SIZE } from "../lib/windowSizes";

import "./MainScreen.css";

/** How long the "buffer size adjusted" notice stays up. */
const DELAY_NOTICE_MS = 5000;

export interface MainScreenProps {
  onSettingsClick?: () => void;
  /** Set in the window of someone helping: who they are helping */
  helper?: { name: string };
}

/** A participant's strip until the backend has told of it. */
const DEFAULT_PEER_STRIP: MixerPeerStrip = { volume: 80, pan: 0, muted: false };
const DEFAULT_LOCAL_STRIP = { volume: 80, pan: 0 };

/** `mixer` with `patch` applied to a participant's strip, if they have one. */
function withPeer(
  mixer: MixerSnapshot,
  peerId: string,
  patch: Partial<MixerPeerStrip>
): MixerSnapshot {
  const current = mixer.peers[peerId];
  if (!current) return mixer;
  return { ...mixer, peers: { ...mixer.peers, [peerId]: { ...current, ...patch } } };
}

export function MainScreen({ onSettingsClick, helper }: MainScreenProps) {
  const { t, i18n } = useTranslation();
  const helping = helper !== undefined;
  // Null until the backend has answered; the app starts by connecting.
  const [session, setSession] = useState<SessionSnapshot | null>(null);
  // The link in the OS could not be read. Shown where a failed step of the
  // session is, until the session changes.
  const [linkError, setLinkError] = useState<string | null>(null);
  const [serverUrl, setServerUrl] = useState("");
  const previousPhase = useRef<SessionSnapshot["phase"] | null>(null);
  const [inviteCode, setInviteCode] = useState("");
  const [detailedLatency, setDetailedLatency] = useState<DetailedLatency | null>(null);
  const [networkStats, setNetworkStats] = useState<NetworkStats | null>(null);
  const [connectionState, setConnectionState] = useState<string | null>(null);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  // Last bandwidth band we warned about, so the toast appears on the edge rather
  // than on every 100ms poll.
  const [warnedBandwidth, setWarnedBandwidth] = useState<string | null>(null);
  // The buffer size is adjusted automatically (REQ-LAT-108); the user is told
  // for a few seconds each time.
  const [delayAdjustments, setDelayAdjustments] = useState(0);
  const [delayNoticeVisible, setDelayNoticeVisible] = useState(false);
  const [inputLevel, setInputLevel] = useState(0);
  const [outputLevel, setOutputLevel] = useState(0);
  const [connectionHistory, setConnectionHistory] = useState<ConnectionHistoryEntry[]>([]);
  const [peerAudio, setPeerAudio] = useState<PeerAudioInfo | null>(null);
  const [localSampleRate, setLocalSampleRate] = useState<number>(48000);
  const [localChannelCount, setLocalChannelCount] = useState<number>(2);
  const [showLeaveDialog, setShowLeaveDialog] = useState(false);
  const [leavePending, setLeavePending] = useState(false);

  // Where the faders stand, as the backend has them (null until it has
  // answered). The microphone's mute is the audio's, read with its status.
  const [mixer, setMixer] = useState<MixerSnapshot | null>(null);
  const [isLocalMuted, setIsLocalMuted] = useState(false);
  // Whether the user hears their own input directly. Off at the start of every
  // session (the backend resets it) - a monitored microphone can feed back.
  const [isMonitoring, setIsMonitoring] = useState(false);

  // The session as the backend has it. An announcement is numbered: an older
  // one arriving after a newer one (or after the first read) is ignored.
  const shownRevision = useRef(-1);
  const showSession = useCallback((next: SessionSnapshot) => {
    if (next.revision < shownRevision.current) return;
    shownRevision.current = next.revision;
    setSession(next);
  }, []);
  // Listen first, then read: a change between the two would be lost the other way round.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listenEvent<SessionSnapshot>(SESSION_CHANGED, showSession)
      .then((stop) => {
        if (cancelled) {
          stop();
          return;
        }
        unlisten = stop;
        return sessionGet().then(showSession);
      })
      .catch((e) => console.error("Failed to read the session:", e));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [showSession]);

  // The faders, the same way: an announcement is numbered, and listening comes first.
  const shownMixerRevision = useRef(-1);
  const showMixer = useCallback((next: MixerSnapshot) => {
    if (next.revision < shownMixerRevision.current) return;
    shownMixerRevision.current = next.revision;
    setMixer(next);
  }, []);
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listenEvent<MixerSnapshot>(MIXER_CHANGED, showMixer)
      .then((stop) => {
        if (cancelled) {
          stop();
          return;
        }
        unlisten = stop;
        return mixerGet().then(showMixer);
      })
      .catch((e) => console.error("Failed to read the mixer:", e));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [showMixer]);

  const phase = session?.phase ?? "connecting_server";
  const room = session?.room ?? null;
  const participants = useMemo(() => room?.participants ?? [], [room]);
  const connectionId = session?.connection_id ?? null;
  const peerName = room?.peer_name ?? "";
  const phaseRef = useRef(phase);
  phaseRef.current = phase;

  // Update html lang attribute when language changes
  useEffect(() => {
    document.documentElement.lang = i18n.language;
  }, [i18n.language]);

  // Resize the main window to match ui.pen's JoinRoom frame (600x700) while
  // disconnected, and to ui.pen's Screens/Main (1134 wide) once connected.
  // Gated on the derived boolean (not the phase directly) so the handful of
  // non-connected phases a single connect attempt passes through
  // (connecting_server -> server_connected -> creating/joining) don't each
  // re-fire an identical, redundant resize.
  const isConnected = phase === "connected";
  const wasConnectedRef = useRef(isConnected);
  useEffect(() => {
    if (helping) return;
    if (wasConnectedRef.current === isConnected) return;
    wasConnectedRef.current = isConnected;
    const target = isConnected ? MIXER_WINDOW_SIZE : JOIN_WINDOW_SIZE;
    const minSize = isConnected ? MIXER_MIN_SIZE : JOIN_MIN_SIZE;
    windowResizeMain(target.width, target.height, minSize.width, minSize.height).catch((e) =>
      console.error("Failed to resize window:", e)
    );
  }, [isConnected, helping]);

  // Which step the app is at decides which buttons do anything, so its
  // transitions are the first thing a bug report needs (ADR-036). The backend
  // logs them too; this is what the screen saw.
  useEffect(() => {
    if (session === null) return;
    const from = previousPhase.current;
    const to = session.phase;
    previousPhase.current = to;
    if (from === to && to !== "error") return;
    const detail = to === "error" ? `: ${session.error}` : "";
    console.info(`[session] ${from ?? "(start)"} -> ${to}${detail}`);
  }, [session]);

  // Any change of the session ends a link error's stay: what the session
  // reports now is newer.
  useEffect(() => {
    setLinkError(null);
  }, [session?.revision]);

  // Load saved configuration. The connection itself is the backend's: it
  // starts as the app does (ADR-024 - the device identity is created and
  // presented by the Rust side without any user interaction).
  useEffect(() => {
    // The rooms a person has been in are theirs: a helper is not shown them.
    if (!helping) {
      configGetConnectionHistory()
        .then(setConnectionHistory)
        .catch((e) => console.log("Failed to load connection history:", e));
    }
    // Load sample rate (ADR-013)
    configGetSampleRate()
      .then(setLocalSampleRate)
      .catch((e) => console.log("Failed to load sample rate, using default:", e));
    // Load transmit channel count (mono/stereo)
    configGetTransmitChannels()
      .then(setLocalChannelCount)
      .catch((e) => console.log("Failed to load transmit channel count, using default:", e));
  }, [helping]);

  // The backend saves a room to the history when it is joined.
  const roomId = room?.room_id;
  useEffect(() => {
    if (roomId === undefined || helping) return;
    configGetConnectionHistory()
      .then(setConnectionHistory)
      .catch((e) => console.log("Failed to load connection history:", e));
  }, [roomId, helping]);

  // Read the URL fresh on every attempt (not just at mount): a retry after
  // changing it in the Settings window must show what it is now dialing, not
  // what it dialed the first time.
  useEffect(() => {
    if (phase !== "connecting_server" || helping) return;
    configGetEffectiveServerUrl()
      .then(setServerUrl)
      .catch((e) => console.log("Failed to load the signaling server URL:", e));
  }, [phase, helping]);

  // Every audio setting change is announced with this (settings.rs, ADR-043),
  // whether the settings window, a helping peer or a test made it, so the
  // mixer's quality badge reflects the change immediately instead of only
  // after an app restart. Mirrors the i18n:language-changed handling in App.tsx.
  // The announcement carries the settings now in effect, numbered: an older
  // one arriving after a newer one is ignored.
  const shownSettingsRevision = useRef(-1);
  const handleAudioConfigChanged = useCallback((settings: AudioSettings) => {
    if (settings.revision < shownSettingsRevision.current) return;
    shownSettingsRevision.current = settings.revision;
    setLocalSampleRate(settings.sample_rate);
    setLocalChannelCount(settings.transmit_channels);
  }, []);
  useWindowEvent<AudioSettings>(AUDIO_SETTINGS_CHANGED, handleAudioConfigChanged);

  const [reconnectPending, setReconnectPending] = useState(false);

  // The prompt only informs unless retrying does something (REQ-CON-110).
  const handleReconnect = useCallback(async () => {
    setReconnectPending(true);
    try {
      await streamingReconnect();
    } catch (e) {
      console.error("Reconnect failed:", e);
    } finally {
      // The state comes back through the poll; clearing here just re-enables the
      // button so a second attempt is possible.
      setReconnectPending(false);
    }
  }, []);

  // The failure of any of these is the session's `error` phase, which is drawn,
  // and the failed call is in the log; there is nothing more for the screen to do.
  const handleCreateRoom = useCallback(() => {
    sessionCreate().catch(() => undefined);
  }, []);
  const handleJoinRoom = useCallback((code: string) => {
    sessionJoin(code).catch(() => undefined);
  }, []);
  const handleReconnectSignaling = useCallback(() => {
    sessionReconnect().catch(() => undefined);
  }, []);

  // An invite link from the OS joins the room it names (REQ-CON-103). A link
  // with a malformed code surfaces as a room-level error instead of being
  // dropped silently, whether it arrived at launch or while running.
  //
  // Depends on there being a connection, because joining needs one: a link
  // clicked before the app finished connecting is handled once it has.
  const hasConnection = connectionId !== null;
  const handleJoinRoomRef = useRef(handleJoinRoom);
  handleJoinRoomRef.current = handleJoinRoom;
  useEffect(() => {
    // A link opened on the helper's machine is not for the helped app to join.
    if (!hasConnection || helping) {
      return;
    }

    let cleanup: (() => void) | undefined;
    let cancelled = false;

    registerInviteLinkHandler(
      (code) => {
        handleJoinRoomRef.current(code);
      },
      () => {
        if (phaseRef.current !== "connected") {
          setLinkError("invalid invite link");
        }
      }
    )
      .then((unlisten) => {
        // The effect may have been torn down while we awaited registration.
        if (cancelled) {
          unlisten();
        } else {
          cleanup = unlisten;
        }
      })
      .catch((e) => {
        // Deep links are a convenience; the invite code field still works.
        console.warn("Could not register the invite link handler:", e);
      });

    return () => {
      cancelled = true;
      cleanup?.();
    };
  }, [hasConnection, helping]);

  // Helping with settings (ADR-043). Its events come from the backend as the
  // room's events are read.
  const settingsHelp = useSettingsHelp(connectionId, participants);
  const settingsHelpEventRef = useRef(settingsHelp.onEvent);
  settingsHelpEventRef.current = settingsHelp.onEvent;
  const handleSettingsHelpEvent = useCallback(
    (event: HelpEvent) => settingsHelpEventRef.current(event),
    []
  );
  useWindowEvent<HelpEvent>(SESSION_SETTINGS_HELP, handleSettingsHelpEvent);

  // Footer indicator inputs. The quality band comes from the core library
  // (REQ-LAT-121); nothing here re-derives it from RTT and loss.
  const indicatorStatus: ConnectionStatus = useMemo(() => {
    if (connectionState === "failed") return "error";
    if (connectionState === "reconnecting") return "unstable";
    if (phase === "connected") return "connected";
    if (phase === "connecting_server") return "connecting";
    return "disconnected";
  }, [connectionState, phase]);

  // Device input/output latency, taken from the breakdown rather than recomputed
  // (REQ-LAT-122).
  const deviceLatency = useMemo(() => {
    if (!detailedLatency) return null;
    const capture = detailedLatency.upstream.find((c) => c.name.includes("Capture"));
    const playback = detailedLatency.downstream.find((c) => c.name.includes("Playback"));
    if (!capture || !playback) return null;
    return { input: capture.ms, output: playback.ms };
  }, [detailedLatency]);

  // Bandwidth warning. Bitrate adaptation is out of scope (ADR-022), so a narrow
  // link is reported rather than worked around.
  //
  // "no_signal" (zero bytes in the last interval) is reported separately from
  // "insufficient": no packets arriving at all is a dead connection, not a
  // narrow link, and must not be worded as a bandwidth problem (REQ-LAT-127).
  const bandwidthWarning = useMemo(() => {
    const status = networkStats?.bandwidth_status;
    if (!status || status === "sufficient") return null;
    if (status === "no_signal") {
      return t("session.bandwidth.noSignal");
    }
    const requiredMbps = ((networkStats?.required_bps ?? 0) / 1_000_000).toFixed(1);
    if (status === "insufficient") {
      return t("session.bandwidth.insufficient", { required: requiredMbps });
    }
    return t("session.bandwidth.marginal");
  }, [networkStats, t]);

  // Warn once per band change rather than on every poll.
  useEffect(() => {
    const status = networkStats?.bandwidth_status ?? null;
    if (status !== warnedBandwidth) {
      if (status && status !== "sufficient") {
        console.warn(`Bandwidth ${status}: ${bandwidthWarning}`);
      }
      setWarnedBandwidth(status);
    }
  }, [networkStats, warnedBandwidth, bandwidthWarning]);

  useEffect(() => {
    if (delayAdjustments === 0) {
      setDelayNoticeVisible(false);
      return;
    }
    setDelayNoticeVisible(true);
    const timer = setTimeout(() => setDelayNoticeVisible(false), DELAY_NOTICE_MS);
    return () => clearTimeout(timer);
  }, [delayAdjustments]);

  // Poll streaming status for latency and audio level when connected
  useEffect(() => {
    if (phase !== "connected") {
      setDetailedLatency(null);
      setNetworkStats(null);
      setConnectionState(null);
      setConnectionError(null);
      setWarnedBandwidth(null);
      setDelayAdjustments(0);
      setInputLevel(0);
      setOutputLevel(0);
      setPeerAudio(null);
      setIsMonitoring(false);
      return;
    }

    // A reading is asked for only once the last has come back: across the relay
    // a reading takes as long as it takes, and asking again meanwhile would only
    // queue readings behind it.
    let reading = false;
    const pollStats = async () => {
      // A helper's window reads the helped app's meters across the relay, so it
      // does so only while it can be seen.
      if (reading || (helping && document.visibilityState === "hidden")) return;
      reading = true;
      try {
        const status = await streamingStatus();
        if (status.is_active) {
          setDetailedLatency(status.latency);
          setNetworkStats(status.network);
          setConnectionState(status.connection_state);
          setConnectionError(status.connection_error);
          setDelayAdjustments(status.audio_quality?.delay_adjustments ?? 0);
          setIsLocalMuted(status.is_muted);
          setIsMonitoring(status.is_monitoring);
          setInputLevel(status.input_level);
          setOutputLevel(status.output_level);
          // Update peer audio info (ADR-013)
          setPeerAudio(status.peer_audio);
        }
      } catch (e) {
        console.error("Failed to get streaming status:", e);
      } finally {
        reading = false;
      }
    };

    // Initial poll
    pollStats();

    // Poll every 100ms for smoother audio level updates
    const interval = setInterval(pollStats, 100);

    return () => clearInterval(interval);
  }, [phase, helping]);

  // Handle settings click
  const handleSettingsClick = () => {
    onSettingsClick?.();
  };

  // Copy the current room invite code to the clipboard
  const handleCopyCode = useCallback((code: string) => {
    navigator.clipboard?.writeText(code).catch((e) =>
      console.error("Failed to copy invite code:", e)
    );
  }, []);

  // Confirm leaving from the leave dialog. Keeps the dialog open in its
  // "pending" (ui.pen Dialog/LeaveConfirm Loading) state until the leave
  // completes - the connected screen (and this dialog with it) then
  // unmounts naturally once the phase flips away from "connected".
  const handleConfirmLeave = async () => {
    setLeavePending(true);
    try {
      await sessionLeave();
      setInviteCode("");
    } catch {
      // What the screen shows is the session's `error` phase.
    } finally {
      setLeavePending(false);
      setShowLeaveDialog(false);
    }
  };

  // Handle selecting from connection history
  const handleHistorySelect = useCallback((roomCode: string) => {
    setInviteCode(roomCode);
  }, []);

  // Handle removing from connection history
  const handleHistoryRemove = useCallback(async (roomCode: string) => {
    try {
      await configRemoveConnectionHistory(roomCode);
      setConnectionHistory((prev) => prev.filter((e) => e.room_code !== roomCode));
    } catch (e) {
      console.error("Failed to remove from history:", e);
    }
  }, []);

  // Handle channel volume change from MixerPanel. The fader moves at once; the
  // backend's announcement then says where it stands.
  const handleChannelVolumeChange = useCallback(async (channelId: string, volume: number) => {
    setMixer((prev) =>
      prev &&
      (channelId === "local"
        ? { ...prev, local: { ...prev.local, volume } }
        : withPeer(prev, channelId, { volume }))
    );
    try {
      showMixer(
        channelId === "local"
          ? await mixerSetLocalVolume(volume)
          : await mixerSetPeerVolume(channelId, volume)
      );
    } catch (e) {
      console.error("Failed to set the volume:", e);
    }
  }, [showMixer]);

  // Handle channel pan change from MixerPanel
  const handleChannelPanChange = useCallback(async (channelId: string, pan: number) => {
    setMixer((prev) =>
      prev &&
      (channelId === "local"
        ? { ...prev, local: { ...prev.local, pan } }
        : withPeer(prev, channelId, { pan }))
    );
    try {
      showMixer(
        channelId === "local"
          ? await mixerSetLocalPan(pan)
          : await mixerSetPeerPan(channelId, pan)
      );
    } catch (e) {
      console.error("Failed to set the pan:", e);
    }
  }, [showMixer]);

  // Handle channel mute toggle from MixerPanel
  const handleChannelMuteToggle = useCallback(async (channelId: string) => {
    if (channelId === "local") {
      try {
        const newMuteState = !isLocalMuted;
        await streamingSetMute(newMuteState);
        setIsLocalMuted(newMuteState);
      } catch (e) {
        console.error("Failed to toggle mute:", e);
      }
    } else {
      try {
        const muted = !(mixer?.peers[channelId] ?? DEFAULT_PEER_STRIP).muted;
        showMixer(await mixerSetPeerMuted(channelId, muted));
      } catch (e) {
        console.error("Failed to toggle mute:", e);
      }
    }
  }, [isLocalMuted, mixer, showMixer]);

  // Handle monitor toggle from the local channel strip
  const handleChannelMonitorToggle = useCallback(async () => {
    try {
      const enabled = !isMonitoring;
      await streamingSetMonitoring(enabled);
      setIsMonitoring(enabled);
    } catch (e) {
      console.error("Failed to toggle monitoring:", e);
    }
  }, [isMonitoring]);

  // Channels array for MixerPanel. useMemo (not useCallback) because this is
  // consumed as a value (`channels={mixerChannels}`), never called as a
  // function — a useCallback here would memoize a function reference that's
  // immediately invoked, which achieves nothing.
  const local = mixer?.local ?? DEFAULT_LOCAL_STRIP;
  const mixerChannels = useMemo((): Channel[] => {
    const channels: Channel[] = [];

    // Local (self) channel
    channels.push({
      id: "local",
      name: peerName,
      type: "local",
      sampleRate: localSampleRate,
      channelCount: localChannelCount,
      levelL: isLocalMuted ? 0 : inputLevel,
      levelR: isLocalMuted ? 0 : inputLevel, // Mono input shown as dual
      volume: local.volume,
      pan: local.pan,
      isMuted: isLocalMuted,
      isMonitoring,
    });

    // Peer channels
    if (phase === "connected") {
      participants.forEach((peer) => {
        const peerState = mixer?.peers[peer.id] ?? DEFAULT_PEER_STRIP;
        channels.push({
          id: peer.id,
          name: peer.name,
          type: "remote",
          sampleRate: peerAudio?.sample_rate ?? 48000,
          channelCount: peerAudio?.channel_count ?? 2,
          levelL: peerState.muted ? 0 : outputLevel,
          levelR: peerState.muted ? 0 : outputLevel,
          volume: peerState.volume,
          pan: peerState.pan,
          isMuted: peerState.muted,
        });
      });
    }

    return channels;
  }, [local, isLocalMuted, isMonitoring, peerName, localSampleRate, localChannelCount, inputLevel, phase, participants, mixer, peerAudio, outputLevel]);

  // Map the session to ConnectionPanel state
  const getConnectionPanelState = (): ConnectionState => {
    if (linkError !== null) return "error";
    switch (phase) {
      case "connecting_server":
      case "creating":
      case "joining":
        return "connecting";
      case "error":
        return "error";
      default:
        return "idle";
    }
  };

  // What went wrong, for the ConnectionPanel
  const failure = linkError ?? (phase === "error" ? (session?.error ?? "") : null);

  // Get error message for ConnectionPanel
  const getErrorMessage = (): string | undefined => {
    if (failure !== null) {
      const formatted = formatErrorForDisplay(failure, t);
      return `${formatted.title}: ${formatted.message}`;
    }
    return undefined;
  };

  // Convert connection history to ConnectionPanel format
  const getConnectionPanelHistory = (): ConnectionPanelHistoryEntry[] => {
    return connectionHistory.map((entry) => ({
      room_code: entry.room_code,
      label: entry.label ?? undefined, // Convert null to undefined
      connected_at: entry.connected_at,
    }));
  };

  // Cancel or retry the connection: drop what there is and connect again. The
  // URL is read again here as well, because cancelling an attempt that is
  // already "connecting" leaves the phase as it was.
  const handleCancelConnection = useCallback(() => {
    configGetEffectiveServerUrl()
      .then(setServerUrl)
      .catch((e) => console.log("Failed to load the signaling server URL:", e));
    sessionConnect().catch(() => undefined);
  }, []);

  // Render connection panel for non-connected states
  const renderConnectionPanel = () => {
    const panelState = getConnectionPanelState();
    const errorMessage = getErrorMessage();
    // A connectionId means the signaling server round-trip that created it
    // already succeeded, so an "error" here is about the room (invalid code,
    // full room), not about reaching the server at all.
    const errorKind: ConnectionErrorKind = connectionId === null ? "server" : "room";

    return (
      <div className="main-connection-panel-wrapper">
        <ConnectionPanel
          state={panelState}
          code={inviteCode}
          errorMessage={errorMessage}
          errorKind={errorKind}
          rawErrorMessage={failure ?? undefined}
          serverUrl={serverUrl}
          connected={connectionId !== null}
          notConnectedReason={t("session.notConnected.reason", "Not connected to the server yet")}
          onCreateRoom={handleCreateRoom}
          onJoinRoom={handleJoinRoom}
          onCodeChange={setInviteCode}
          onCancel={handleCancelConnection}
          onRetry={handleCancelConnection}
          onOpenSettings={handleSettingsClick}
          connectionHistory={getConnectionPanelHistory()}
          onHistorySelect={handleHistorySelect}
          onHistoryRemove={handleHistoryRemove}
          title="jamjam"
          welcomeTitle={t("session.welcome.title")}
          welcomeSubtitle={t("session.welcome.subtitle")}
          createRoomText={t("session.create.button")}
          orText={t("common.label.or")}
          codeLabel={t("session.invite.joinByCode", "Invite Code")}
          codePlaceholder={t("session.join.placeholder")}
          joinText={t("session.join.button")}
          connectingText={
            phase === "connecting_server"
              ? t("signaling.connecting", "Connecting to server...")
              : phase === "creating"
                ? t("session.create.loading")
                : t("session.join.loading")
          }
          cancelText={t("common.button.cancel", "Cancel")}
          historyTitle={t("connectionHistory.title")}
          testRoomCode={session?.test_room_invite_code ?? undefined}
          testRoomTitle={t("session.testRoom.title")}
          testRoomDescription={t("session.testRoom.description")}
        />
      </div>
    );
  };

  // Render the connected session screen: a 3-column layout
  // (room sidebar | mixer | chat) with a header and status footer,
  // matching ui.pen Screens/Main.
  const renderConnectedState = () => {
    if (phase !== "connected") return null;

    // The invite code as the server reported it. No fallback derived from the
    // room's UUID: those six characters look exactly like an invite code but
    // no room can be joined with them, so a participant who copied it could
    // not invite anyone (ADR-026).
    const roomCode = room?.invite_code ?? "";
    const participantCount = participants.length + 1;
    const upMs = detailedLatency ? Math.round(detailedLatency.upstream_total_ms) : null;
    const downMs = detailedLatency ? Math.round(detailedLatency.downstream_total_ms) : null;

    return (
      <>
        <header className="main-header">
          <span className="main-header__logo">jamjam</span>
          <div className="main-header__actions">
            <button
              type="button"
              className="main-header__icon-btn"
              onClick={handleSettingsClick}
              aria-label={t("settings.title")}
              title={t("settings.title")}
            >
              <SettingsIcon />
            </button>
          </div>
        </header>

        <div className="main-body" data-testid="session">
          {/* Room / participants sidebar */}
          <aside className="room-sidebar" data-testid="room-sidebar">
            <div className="room-sidebar__room">
              <span className="room-sidebar__room-label">{t("session.room.label")}</span>
              <div className="room-sidebar__code">
                <span className="room-sidebar__code-text" data-testid="room-code">{roomCode}</span>
                <button
                  type="button"
                  className="room-sidebar__copy"
                  onClick={() => handleCopyCode(roomCode)}
                  aria-label={t("session.invite.copy")}
                  title={t("session.invite.copy")}
                >
                  <CopyIcon />
                </button>
              </div>
            </div>

            <div className="room-sidebar__divider" />

            <div className="room-sidebar__participants">
              <span className="room-sidebar__participants-label">
                {t("session.participant.title", { count: participantCount })}
              </span>
              <ul className="room-sidebar__participant-list" data-testid="participant-list">
                <li className="participant participant--self">
                  <span className="participant__info">
                    <span className="participant__icon participant__icon--self">
                      <UserIcon />
                    </span>
                    <span className="participant__name">{peerName}</span>
                  </span>
                </li>
                {participants.map((participant) => (
                  <li key={participant.id} className="participant">
                    <span className="participant__info">
                      <span className="participant__icon">
                        <UserIcon />
                      </span>
                      <span className="participant__name">{participant.name}</span>
                    </span>
                    {!helping && settingsHelp.canOffer(participant) && (
                      <button
                        type="button"
                        className="participant__help"
                        data-testid="settings-help-offer"
                        data-peer-name={participant.name}
                        onClick={() => settingsHelp.offer(participant.id)}
                        aria-label={t("settingsHelp.offerLabel", { name: participant.name })}
                        title={t("settingsHelp.offerLabel", { name: participant.name })}
                      >
                        {t("settingsHelp.offer")}
                      </button>
                    )}
                    {upMs !== null && downMs !== null && (
                      <span className="participant__latency">
                        <span className="participant__latency-item">
                          <span className="participant__latency-arrow">↑</span>
                          {upMs}ms
                        </span>
                        <span className="participant__latency-item">
                          <span className="participant__latency-arrow">↓</span>
                          {downMs}ms
                        </span>
                      </span>
                    )}
                  </li>
                ))}
              </ul>
            </div>

            <div className="room-sidebar__divider" />

            <MasterSection levelL={outputLevel} levelR={outputLevel} />

            {/* Leaving is the helped person's own (ADR-044 §3). */}
            {!helping && (
              <button
                type="button"
                className="room-sidebar__leave"
                data-testid="leave-room"
                onClick={() => setShowLeaveDialog(true)}
              >
                <span>{t("session.leave.button")}</span>
                <LogOutIcon />
              </button>
            )}
          </aside>

          {/* Mixer */}
          <div className="main-mixer-column">
            {settingsHelp.bars}
            <MixerPanel
              channels={mixerChannels}
              onChannelVolumeChange={handleChannelVolumeChange}
              onChannelPanChange={handleChannelPanChange}
              onChannelMuteToggle={handleChannelMuteToggle}
              onChannelMonitorToggle={handleChannelMonitorToggle}
            />
          </div>

          {/* Chat (docked column). Speaking is the helped person's own (ADR-044 §3). */}
          {!helping && (
            <div className="main-chat-column">
              <ChatPanelAdapter connId={connectionId} myPeerId={room?.peer_id ?? null} />
            </div>
          )}
        </div>

        <footer className="main-footer">
          <div className="main-footer__status">
            <ConnectionIndicator
              status={indicatorStatus}
              quality={networkStats?.quality ?? undefined}
              latencyMs={networkStats?.rtt_ms ?? undefined}
              inputLatencyMs={deviceLatency?.input}
              outputLatencyMs={deviceLatency?.output}
            />
          </div>
          {delayNoticeVisible && (
            <div className="main-footer__warning">
              <Toast
                type="success"
                message={t("notification.delayAdjusted")}
              />
            </div>
          )}
          {bandwidthWarning && (
            <div className="main-footer__warning">
              <Toast type="warning" message={bandwidthWarning} />
            </div>
          )}
          {connectionState === "failed" && (
            <div className="main-footer__warning">
              <Toast
                type="error"
                message={t("session.reconnect.prompt", {
                  reason: connectionError ?? "",
                })}
              />
              <button
                type="button"
                className="main-footer__reconnect"
                onClick={handleReconnect}
                disabled={reconnectPending}
              >
                {t("session.reconnect.retry")}
              </button>
            </div>
          )}
          {session?.signaling_reconnect === "reconnecting" && (
            <div className="main-footer__warning">
              <Toast type="warning" message={t("session.signalingReconnect.inProgress")} />
            </div>
          )}
          {session?.signaling_reconnect === "failed" && (
            <div className="main-footer__warning">
              <Toast
                type="error"
                message={t("session.signalingReconnect.failed", {
                  reason: session.signaling_reconnect_error ?? "",
                })}
              />
              <button
                type="button"
                className="main-footer__reconnect"
                onClick={handleReconnectSignaling}
              >
                {t("session.reconnect.retry")}
              </button>
            </div>
          )}
        </footer>

        {settingsHelp.overlays}

        <LeaveDialog
          open={showLeaveDialog}
          pending={leavePending}
          onConfirm={handleConfirmLeave}
          onCancel={() => setShowLeaveDialog(false)}
          title={t("session.leave.confirmMessage")}
          pendingTitle={t("session.leave.pending")}
          confirmLabel={t("session.leave.confirmButton")}
          cancelLabel={t("common.button.cancel")}
        />
      </>
    );
  };

  // Check if we should show connection panel (not connected to a room)
  const showConnectionPanel = phase !== "connected";

  // A helper has nothing to join or create: until the helped app is in its room
  // there is only this.
  if (helping && showConnectionPanel) {
    return (
      <div className="main-screen">
        <main className="main-content main-content--flush">
          <p className="main-helper-waiting" role="status" data-testid="settings-help-window-waiting">
            {t("settingsHelp.window.waiting", { name: helper.name })}
          </p>
        </main>
      </div>
    );
  }

  return (
    <div className="main-screen">
      {showConnectionPanel ? (
        <main className="main-content main-content--flush">
          {renderConnectionPanel()}
        </main>
      ) : (
        renderConnectedState()
      )}
    </div>
  );
}

function SettingsIcon() {
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

function CopyIcon() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
      <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
    </svg>
  );
}

function UserIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2" />
      <circle cx="12" cy="7" r="4" />
    </svg>
  );
}

function LogOutIcon() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
      <polyline points="16 17 21 12 16 7" />
      <line x1="21" x2="9" y1="12" y2="12" />
    </svg>
  );
}

export default MainScreen;
