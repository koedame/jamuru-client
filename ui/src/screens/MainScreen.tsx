/**
 * Main Screen
 *
 * Entry point for session creation and joining.
 * Displays connection status and provides room management UI.
 */
import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { ConnectionPanel, type ConnectionState, type ConnectionErrorKind, type ConnectionHistoryEntry as ConnectionPanelHistoryEntry } from "../components/ConnectionPanel";
import { MixerPanel, MasterSection, type Channel } from "../components/MixerPanel";
import { ChatPanelAdapter } from "../components/ChatPanel";
import { ConnectionIndicator, type ConnectionStatus } from "../components/ConnectionIndicator";
import { Toast } from "../components/Toast";
import { LeaveDialog } from "../components/LeaveDialog";
import { formatErrorForDisplay } from "../lib/errorMessages";
import { registerInviteLinkHandler } from "../lib/deepLink";
import { testRoomCodeOf } from "../lib/inviteCode";
import { useWindowEvent } from "../hooks/useWindowEvents";
import {
  signalingConnect,
  signalingDisconnect,
  signalingListRooms,
  signalingJoinRoom,
  signalingLeaveRoom,
  signalingCreateRoom,
  signalingPollEvents,
  signalingPublishLocalCandidates,
  streamingPrepare,
  streamingStart,
  peerSortedAddrs,
  streamingStop,
  streamingReconnect,
  streamingStatus,
  streamingSetMute,
  streamingSetMonitoring,
  audioGetCurrentDevices,
  audioGetBufferSize,
  streamingSetPeerVolume,
  streamingSetPeerPan,
  streamingSetLocalVolume,
  streamingSetLocalPan,
  configGetConnectionHistory,
  configAddConnectionHistory,
  configRemoveConnectionHistory,
  configGetPeerName,
  configGetSampleRate,
  configGetTransmitChannels,
  configGetEffectiveServerUrl,
  windowResizeMain,
  type RoomInfo,
  type NetworkStats,
  type PeerInfo,
  type DetailedLatency,
  type ConnectionHistoryEntry,
  type PeerAudioInfo,
} from "../lib/tauri";
import { JOIN_WINDOW_SIZE, MIXER_WINDOW_SIZE, JOIN_MIN_SIZE, MIXER_MIN_SIZE } from "../lib/windowSizes";

import "./MainScreen.css";

/** How long the "buffer size adjusted" notice stays up. */
const DELAY_NOTICE_MS = 5000;

/** How many times to retry the signaling connection before giving up and
 * asking the user to retry manually. */
const MAX_SIGNALING_RECONNECT_ATTEMPTS = 5;
/** Delay before retry N: N * this value, so attempts back off (2s, 4s, 6s...). */
const SIGNALING_RECONNECT_BASE_DELAY_MS = 2000;

// Session state type
type SessionState =
  | { status: "idle" }
  | { status: "connecting_server" }
  | { status: "server_connected"; rooms: RoomInfo[] }
  | { status: "creating" }
  | { status: "joining"; code: string }
  | { status: "connected"; roomCode: string; participants: PeerInfo[] }
  | { status: "error"; message: string };

export interface MainScreenProps {
  onSettingsClick?: () => void;
}

interface ChannelState {
  volume: number;
  pan: number;
  isMuted: boolean;
}

/** Clone-and-patch one peer's channel state; no-op if the peer isn't tracked yet. */
function withPeerChannelPatch(
  prev: Map<string, ChannelState>,
  channelId: string,
  patch: Partial<ChannelState>
): Map<string, ChannelState> {
  const current = prev.get(channelId);
  if (!current) return prev;
  const next = new Map(prev);
  next.set(channelId, { ...current, ...patch });
  return next;
}

export function MainScreen({ onSettingsClick }: MainScreenProps) {
  const { t, i18n } = useTranslation();
  const [sessionState, setSessionState] = useState<SessionState>({ status: "connecting_server" });
  const [connectionId, setConnectionId] = useState<number | null>(null);
  // The server says which room to offer as the test room, and to whom; null
  // hides the shortcut.
  const [testRoomCode, setTestRoomCode] = useState<string | null>(null);
  const [peerName, setPeerName] = useState("User");
  const [serverUrl, setServerUrl] = useState("");
  const hasAutoConnected = useRef(false);
  const previousStatus = useRef<SessionState["status"] | null>(null);
  const [inviteCode, setInviteCode] = useState("");
  const [currentInviteCode, setCurrentInviteCode] = useState("");
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
  const [myPeerId, setMyPeerId] = useState<string | null>(null);
  const [peerAudio, setPeerAudio] = useState<PeerAudioInfo | null>(null);
  const [localSampleRate, setLocalSampleRate] = useState<number>(48000);
  const [localChannelCount, setLocalChannelCount] = useState<number>(2);
  const [showLeaveDialog, setShowLeaveDialog] = useState(false);
  const [leavePending, setLeavePending] = useState(false);
  // Reconnecting to the signaling server after an unexpected WebSocket
  // disconnect, shown while in a room. Distinct from
  // `reconnectPending`/`handleReconnect` below, which re-establishes the P2P
  // audio link (ADR-022/REQ-CON-110) rather than the signaling connection.
  const [signalingReconnectState, setSignalingReconnectState] = useState<
    "idle" | "reconnecting" | "failed"
  >("idle");
  const [signalingReconnectError, setSignalingReconnectError] = useState<string | null>(null);

  // Mixer channel states for MixerPanel
  const [localChannelState, setLocalChannelState] = useState<ChannelState>({
    volume: 80,
    pan: 0,
    isMuted: false,
  });
  const [peerChannelStates, setPeerChannelStates] = useState<Map<string, ChannelState>>(new Map());
  // Whether the user hears their own input directly. Off at the start of every
  // session (the backend resets it) - a monitored microphone can feed back.
  const [isMonitoring, setIsMonitoring] = useState(false);

  // Update html lang attribute when language changes
  useEffect(() => {
    document.documentElement.lang = i18n.language;
  }, [i18n.language]);

  // Resize the main window to match ui.pen's JoinRoom frame (600x700) while
  // disconnected, and to ui.pen's Screens/Main (1134 wide) once connected.
  // Gated on the derived boolean (not sessionState.status directly) so the
  // handful of non-connected statuses a single connect attempt passes
  // through (connecting_server -> server_connected -> creating/joining)
  // don't each re-fire an identical, redundant resize.
  const isConnected = sessionState.status === "connected";
  const wasConnectedRef = useRef(isConnected);
  useEffect(() => {
    if (wasConnectedRef.current === isConnected) return;
    wasConnectedRef.current = isConnected;
    const target = isConnected ? MIXER_WINDOW_SIZE : JOIN_WINDOW_SIZE;
    const minSize = isConnected ? MIXER_MIN_SIZE : JOIN_MIN_SIZE;
    windowResizeMain(target.width, target.height, minSize.width, minSize.height).catch((e) =>
      console.error("Failed to resize window:", e)
    );
  }, [isConnected]);

  // Which state the screen is in decides which buttons do anything, so its
  // transitions are the first thing a bug report needs (ADR-036).
  useEffect(() => {
    const from = previousStatus.current;
    const to = sessionState.status;
    previousStatus.current = to;
    if (from === to && to !== "error") return;
    const detail = sessionState.status === "error" ? `: ${sessionState.message}` : "";
    console.info(`[session] ${from ?? "(start)"} -> ${to}${detail}`);
  }, [sessionState]);

  // Load saved configuration and auto-connect to the signaling server.
  // Runs straight from the mount effect below: there is no sign-in step
  // (ADR-024 - the device identity is created and presented by the Rust
  // side without any user interaction).
  const loadConfigAndConnect = async () => {
    // Load peer name from settings
    try {
      const savedPeerName = await configGetPeerName();
      setPeerName(savedPeerName);
    } catch (e) {
      console.log("Failed to load peer name, using default:", e);
    }

    // Load connection history
    try {
      const history = await configGetConnectionHistory();
      setConnectionHistory(history);
    } catch (e) {
      console.log("Failed to load connection history:", e);
    }

    // Load sample rate (ADR-013)
    try {
      const sampleRate = await configGetSampleRate();
      setLocalSampleRate(sampleRate);
    } catch (e) {
      console.log("Failed to load sample rate, using default:", e);
    }

    // Load transmit channel count (mono/stereo)
    try {
      const channelCount = await configGetTransmitChannels();
      setLocalChannelCount(channelCount);
    } catch (e) {
      console.log("Failed to load transmit channel count, using default:", e);
    }

    // Auto-connect to signaling server (only once)
    if (!hasAutoConnected.current) {
      hasAutoConnected.current = true;
      await autoConnect();
    }
  };

  // On mount: load settings and connect immediately. No account gate.
  useEffect(() => {
    loadConfigAndConnect();
  }, []);

  // Every audio setting change is announced with this (settings.rs, ADR-043),
  // whether the settings window, a helping peer or a test made it, so the
  // mixer's quality badge reflects the change immediately instead of only
  // after an app restart. Mirrors the i18n:language-changed handling in App.tsx.
  const handleAudioConfigChanged = useCallback(async () => {
    try {
      const sampleRate = await configGetSampleRate();
      setLocalSampleRate(sampleRate);
    } catch (e) {
      console.log("Failed to reload sample rate:", e);
    }
    try {
      const channelCount = await configGetTransmitChannels();
      setLocalChannelCount(channelCount);
    } catch (e) {
      console.log("Failed to reload transmit channel count:", e);
    }
  }, []);
  useWindowEvent<void>("audio:config-changed", handleAudioConfigChanged);

  // Auto-connect to signaling server
  const autoConnect = async () => {
    setSessionState({ status: "connecting_server" });
    // The shortcut belongs to the server that listed it; until a list comes
    // back from the one being dialed now, there is none to offer.
    setTestRoomCode(null);

    // Read the URL fresh on every attempt (not just at mount): a retry after
    // changing it in the Settings window must show what it is now dialing,
    // not what it dialed the first time.
    try {
      setServerUrl(await configGetEffectiveServerUrl());
    } catch (e) {
      console.log("Failed to load the signaling server URL:", e);
    }

    try {
      const connId = await signalingConnect();
      setConnectionId(connId);
      const rooms = await signalingListRooms(connId);
      setTestRoomCode(testRoomCodeOf(rooms));
      setSessionState({ status: "server_connected", rooms });
    } catch (e) {
      setSessionState({
        status: "error",
        message: String(e),
      });
    }
  };

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (connectionId !== null) {
        signalingDisconnect(connectionId).catch(console.error);
      }
    };
  }, [connectionId]);

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

  // An invite link from the OS joins the room it names (REQ-CON-103). A link
  // with a malformed code surfaces as a room-level error instead of being
  // dropped silently, whether it arrived at launch or while running.
  //
  // Depends on connectionId, because joining needs a signaling connection: a
  // link clicked before the app finished connecting is handled once it has.
  const handleJoinRoomRef = useRef<(roomId: string) => void>(() => {});
  useEffect(() => {
    if (connectionId === null) {
      return;
    }

    let cleanup: (() => void) | undefined;
    let cancelled = false;

    registerInviteLinkHandler(
      (code) => {
        handleJoinRoomRef.current(code);
      },
      () => {
        setSessionState({ status: "error", message: "invalid invite link" });
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
  }, [connectionId]);

  // Footer indicator inputs. The quality band comes from the core library
  // (REQ-LAT-121); nothing here re-derives it from RTT and loss.
  const indicatorStatus: ConnectionStatus = useMemo(() => {
    if (connectionState === "failed") return "error";
    if (connectionState === "reconnecting") return "unstable";
    if (sessionState.status === "connected") return "connected";
    if (sessionState.status === "connecting_server") return "connecting";
    return "disconnected";
  }, [connectionState, sessionState.status]);

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
    if (sessionState.status !== "connected") {
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

    const pollStats = async () => {
      try {
        const status = await streamingStatus();
        if (status.is_active) {
          setDetailedLatency(status.latency);
          setNetworkStats(status.network);
          setConnectionState(status.connection_state);
          setConnectionError(status.connection_error);
          setDelayAdjustments(status.audio_quality?.delay_adjustments ?? 0);
          setLocalChannelState((prev) => ({ ...prev, isMuted: status.is_muted }));
          setIsMonitoring(status.is_monitoring);
          setInputLevel(status.input_level);
          setOutputLevel(status.output_level);
          // Update peer audio info (ADR-013)
          setPeerAudio(status.peer_audio);
        }
      } catch (e) {
        console.error("Failed to get streaming status:", e);
      }
    };

    // Initial poll
    pollStats();

    // Poll every 100ms for smoother audio level updates
    const interval = setInterval(pollStats, 100);

    return () => clearInterval(interval);
  }, [sessionState.status]);

  // The peer streamingStart was called for, or null while not streaming. Set
  // so the peer watcher below does not start a second audio thread when
  // further PeerUpdated events arrive, and so a PeerLeft can tell whether the
  // audio session is with the peer that left.
  const streamingPeerIdRef = useRef<string | null>(null);
  // The room's participants, kept in step with `sessionState` for the event
  // poller, which outlives the render it was created in and handles several
  // events per poll before React re-renders.
  const participantsRef = useRef<PeerInfo[]>([]);
  participantsRef.current = sessionState.status === "connected" ? sessionState.participants : [];
  const updateParticipants = (update: (peers: PeerInfo[]) => PeerInfo[]) => {
    participantsRef.current = update(participantsRef.current);
    setSessionState((prev) =>
      prev.status === "connected" ? { ...prev, participants: update(prev.participants) } : prev
    );
  };

  /// Advertise our audio address to the room.
  ///
  /// Nothing can send us audio until we do: our port is chosen when the socket
  /// is bound, and peers learn it only from this (ADR-026). Runs right after
  /// entering a room, for both the creator and the joiner.
  const publishOwnAddress = async (connId: number) => {
    try {
      const localAddr = await streamingPrepare();
      const port = Number(localAddr.split(":").pop());
      if (!Number.isFinite(port) || port === 0) {
        throw new Error(`unexpected local audio address: ${localAddr}`);
      }
      await signalingPublishLocalCandidates(connId, port);
    } catch (e) {
      // Not fatal for chat or the participant list, which go through the
      // signaling server; only audio depends on this.
      console.error("Failed to publish our audio address:", e);
    }
  };

  /// Start streaming to `peers` if any of them has published an address.
  ///
  /// Called both on entering a room (a peer may already have published) and
  /// from the PeerUpdated handler (a peer publishing later), because whichever
  /// side joins first will only learn the other's address afterwards.
  const startStreamingToPeer = async (peers: PeerInfo[]) => {
    if (streamingPeerIdRef.current !== null) return;

    const peerWithAddr = peers.find((p) => p.public_addr || p.local_addr || p.candidates.length > 0);
    if (!peerWithAddr) return;
    const candidates = peerSortedAddrs(peerWithAddr);
    const addr = candidates[0];
    if (!addr) return;

    streamingPeerIdRef.current = peerWithAddr.id;
    try {
      const [devices, bufferSize] = await Promise.all([
        audioGetCurrentDevices(),
        audioGetBufferSize(),
      ]);
      await streamingStart(
        addr,
        candidates,
        devices.input_device_id ?? undefined,
        devices.output_device_id ?? undefined,
        bufferSize
      );
      console.log("Streaming started to:", addr, "candidates:", candidates);
    } catch (streamErr) {
      // Allow a later PeerUpdated to retry rather than leaving the session
      // permanently silent.
      streamingPeerIdRef.current = null;
      console.error("Failed to start streaming:", streamErr);
    }
  };

  // Reconnect the signaling WebSocket after it drops unexpectedly.
  // `roomToRejoin` is the invite code of the room we were in when the
  // connection was lost, or "" if we had not joined one yet.
  //
  // Retries with a backoff up to MAX_SIGNALING_RECONNECT_ATTEMPTS. While a
  // room is open the room UI stays up and a footer banner reports progress
  // (renderConnectedState below); otherwise the connection screen shows the
  // ordinary "connecting" state and, on giving up, the same server-error
  // screen shown for an initial connect failure.
  const attemptSignalingReconnect = useCallback(
    async (roomToRejoin: string) => {
      const wasInRoom = roomToRejoin !== "";
      setTestRoomCode(null);
      if (wasInRoom) {
        setSignalingReconnectState("reconnecting");
        setSignalingReconnectError(null);
      } else {
        setSessionState({ status: "connecting_server" });
      }

      for (let attempt = 1; attempt <= MAX_SIGNALING_RECONNECT_ATTEMPTS; attempt++) {
        try {
          setServerUrl(await configGetEffectiveServerUrl());
        } catch (e) {
          console.log("Failed to reload the signaling server URL:", e);
        }

        try {
          const newConnId = await signalingConnect();
          if (wasInRoom) {
            const result = await signalingJoinRoom(newConnId, roomToRejoin, peerName);
            setCurrentInviteCode(result.invite_code || roomToRejoin);
            setMyPeerId(result.peer_id);
            setConnectionId(newConnId);
            setSessionState({
              status: "connected",
              roomCode: result.room_id,
              participants: result.peers,
            });
            streamingPeerIdRef.current = null;
            await publishOwnAddress(newConnId);
            await startStreamingToPeer(result.peers);
            setSignalingReconnectState("idle");
          } else {
            const rooms = await signalingListRooms(newConnId);
            setConnectionId(newConnId);
            setTestRoomCode(testRoomCodeOf(rooms));
            setSessionState({ status: "server_connected", rooms });
          }
          return;
        } catch (e) {
          console.warn(
            `Signaling reconnect attempt ${attempt}/${MAX_SIGNALING_RECONNECT_ATTEMPTS} failed:`,
            e
          );
          if (attempt === MAX_SIGNALING_RECONNECT_ATTEMPTS) {
            if (wasInRoom) {
              setSignalingReconnectState("failed");
              setSignalingReconnectError(String(e));
            } else {
              setSessionState({ status: "error", message: String(e) });
            }
            return;
          }
          await new Promise((resolve) =>
            setTimeout(resolve, SIGNALING_RECONNECT_BASE_DELAY_MS * attempt)
          );
        }
      }
    },
    [peerName]
  );

  // Poll signaling events (for peer join/leave, chat, and connection loss).
  // Runs whenever we hold a connection, not only while in a room: a drop
  // while merely connected to the server (browsing the room list) needs the
  // same detection.
  useEffect(() => {
    if (connectionId === null) {
      return;
    }

    const pollEvents = async () => {
      try {
        const events = await signalingPollEvents(connectionId);
        for (const event of events) {
          if (event.type === "PeerJoined") {
            // Avoid duplicate
            updateParticipants((peers) =>
              peers.some((p) => p.id === event.peer.id) ? peers : [...peers, event.peer]
            );
          } else if (event.type === "PeerUpdated") {
            // A peer published (or changed) its audio address. This is what
            // lets whichever side joined first start streaming - at join time
            // the other peer had no address yet (ADR-026).
            updateParticipants((peers) =>
              peers.map((p) => (p.id === event.peer.id ? event.peer : p))
            );
            await startStreamingToPeer([event.peer]);
          } else if (event.type === "PeerLeft") {
            updateParticipants((peers) => peers.filter((p) => p.id !== event.peer_id));
            if (streamingPeerIdRef.current === event.peer_id) {
              // The audio session was with the peer that left: end it rather
              // than let it report a lost connection, and free the slot for
              // whoever is still here or joins next. Starting audio used up the
              // advertised socket, so advertise a fresh one first.
              try {
                await streamingStop();
              } catch (streamErr) {
                console.error("Failed to stop streaming after the peer left:", streamErr);
              }
              streamingPeerIdRef.current = null;
              await publishOwnAddress(connectionId);
              await startStreamingToPeer(participantsRef.current);
            }
          } else if (event.type === "RoomClosed" || (event.type === "Kicked" && event.peer_id === myPeerId)) {
            // The signaling server closes this connection right after
            // sending either message, so `connectionId` is now dead - reusing it for
            // leave/create/join would just fail. Tear it down and get a
            // fresh connection instead of only resetting local UI state.
            console.log(`Room session ended (${event.type}): ${event.reason}`);
            try {
              await streamingStop();
            } catch (streamErr) {
              console.error("Failed to stop streaming after room ended:", streamErr);
            }
            await signalingDisconnect(connectionId).catch(console.error);
            setCurrentInviteCode("");
            setMyPeerId(null);
            setConnectionId(null);
            await autoConnect();
            return;
          } else if (event.type === "ConnectionLost") {
            // The server didn't close the room first (network blip, proxy
            // reset, server restart) - this conn_id is dead either way.
            // Disconnect it and reconnect, rejoining the room we were in.
            console.warn(`Signaling connection lost: ${event.reason}`);
            const roomToRejoin = currentInviteCode;
            try {
              await streamingStop();
            } catch (streamErr) {
              console.error("Failed to stop streaming after connection loss:", streamErr);
            }
            await signalingDisconnect(connectionId).catch(console.error);
            setConnectionId(null);
            void attemptSignalingReconnect(roomToRejoin);
            return;
          }
          // ChatMessageReceived events are handled by ChatPanel's own polling
        }
      } catch (e) {
        console.error("Failed to poll signaling events:", e);
      }
    };

    // Poll every 500ms
    const interval = setInterval(pollEvents, 500);

    return () => clearInterval(interval);
  }, [connectionId, myPeerId, currentInviteCode, attemptSignalingReconnect]);

  // Handle room creation
  const handleCreateRoom = async () => {
    if (connectionId === null) {
      setSessionState({ status: "error", message: "Not connected to the signaling server" });
      return;
    }

    setSessionState({ status: "creating" });

    try {
      const result = await signalingCreateRoom(
        connectionId,
        "My Room",
        peerName
      );
      setCurrentInviteCode(result.invite_code);
      setMyPeerId(result.peer_id);
      setSessionState({
        status: "connected",
        roomCode: result.room_id,
        participants: result.peers,
      });
      streamingPeerIdRef.current = null;
      await publishOwnAddress(connectionId);
      await startStreamingToPeer(result.peers);
    } catch (e) {
      setSessionState({
        status: "error",
        message: String(e),
      });
    }
  };

  // Handle room join
  // Held in a ref so the deep-link listener does not need re-registering every
  // time the handler identity changes.
  const handleJoinRoom = async (roomId: string) => {
    if (connectionId === null) {
      setSessionState({ status: "error", message: "Not connected to the signaling server" });
      return;
    }

    setSessionState({ status: "joining", code: roomId });

    try {
      const result = await signalingJoinRoom(connectionId, roomId, peerName);
      setCurrentInviteCode(result.invite_code || "");
      setMyPeerId(result.peer_id);
      setSessionState({
        status: "connected",
        roomCode: result.room_id,
        participants: result.peers,
      });

      // Save to connection history under the code the user could join with
      // again. `roomId` is what they typed, which is either that code already
      // or a room UUID from a deep link.
      const historyCode = result.invite_code || roomId;
      try {
        await configAddConnectionHistory(historyCode);
        // Reload history to show updated list
        const history = await configGetConnectionHistory();
        setConnectionHistory(history);
      } catch (historyErr) {
        console.error("Failed to save to history:", historyErr);
      }

      // Advertise where we can be reached, then start streaming if a peer has
      // already advertised theirs. A peer that publishes later is picked up by
      // the PeerUpdated handler (ADR-026).
      streamingPeerIdRef.current = null;
      await publishOwnAddress(connectionId);
      await startStreamingToPeer(result.peers);
    } catch (e) {
      setSessionState({
        status: "error",
        message: String(e),
      });
    }
  };

  handleJoinRoomRef.current = handleJoinRoom;

  // Handle leave room
  const handleLeaveRoom = async () => {
    if (connectionId === null) {
      setSessionState({ status: "error", message: "Not connected to the signaling server" });
      return;
    }

    try {
      // Stop streaming first
      try {
        await streamingStop();
        console.log("Streaming stopped");
      } catch (streamErr) {
        console.error("Failed to stop streaming:", streamErr);
      }

      await signalingLeaveRoom(connectionId);
      setCurrentInviteCode("");
      setInviteCode("");
      setMyPeerId(null);
      const rooms = await signalingListRooms(connectionId);
      setTestRoomCode(testRoomCodeOf(rooms));
      setSessionState({ status: "server_connected", rooms });
    } catch (e) {
      setSessionState({
        status: "error",
        message: String(e),
      });
    }
  };

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
  // unmounts naturally once sessionState flips away from "connected".
  const handleConfirmLeave = async () => {
    setLeavePending(true);
    await handleLeaveRoom();
    setLeavePending(false);
    setShowLeaveDialog(false);
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

  // Handle channel volume change from MixerPanel
  const handleChannelVolumeChange = useCallback(async (channelId: string, volume: number) => {
    if (channelId === "local") {
      setLocalChannelState((prev) => ({ ...prev, volume }));
      try {
        // Convert 0-100 fader range to 0-200 backend range (100 = unity)
        const backendVolume = Math.round(volume * 2);
        await streamingSetLocalVolume(backendVolume);
      } catch (e) {
        console.error("Failed to set local volume:", e);
      }
    } else {
      // Peer channel. Peer mute is implemented as backend volume 0 (there is
      // no separate mute flag on the wire), so while muted keep sending 0 —
      // otherwise dragging the fader would audibly un-mute it even though
      // the UI still shows "muted". The stored volume still updates so it
      // takes effect immediately once un-muted.
      let effectiveVolume = volume;
      setPeerChannelStates((prev) => {
        const current = prev.get(channelId);
        if (!current) return prev;
        effectiveVolume = current.isMuted ? 0 : volume;
        return withPeerChannelPatch(prev, channelId, { volume });
      });
      try {
        // Convert 0-100 fader range to 0-200 backend range (100 = unity)
        const backendVolume = Math.round(effectiveVolume * 2);
        await streamingSetPeerVolume(backendVolume);
      } catch (e) {
        console.error("Failed to set peer volume:", e);
      }
    }
  }, []);

  // Handle channel pan change from MixerPanel
  const handleChannelPanChange = useCallback(async (channelId: string, pan: number) => {
    if (channelId === "local") {
      setLocalChannelState((prev) => ({ ...prev, pan }));
      try {
        await streamingSetLocalPan(pan);
      } catch (e) {
        console.error("Failed to set local pan:", e);
      }
    } else {
      setPeerChannelStates((prev) => withPeerChannelPatch(prev, channelId, { pan }));
      try {
        await streamingSetPeerPan(pan);
      } catch (e) {
        console.error("Failed to set peer pan:", e);
      }
    }
  }, []);

  // Handle channel mute toggle from MixerPanel
  const handleChannelMuteToggle = useCallback(async (channelId: string) => {
    if (channelId === "local") {
      // Toggle local mute through backend
      try {
        const newMuteState = !localChannelState.isMuted;
        await streamingSetMute(newMuteState);
        setLocalChannelState((prev) => ({ ...prev, isMuted: newMuteState }));
      } catch (e) {
        console.error("Failed to toggle mute:", e);
      }
    } else {
      // Peer mute - update local state (mute is done by setting volume to 0).
      // The IPC call happens after setState returns, not inside the updater:
      // updaters must stay pure (React invokes them twice in StrictMode/dev).
      let effectiveVolume: number | null = null;
      setPeerChannelStates((prev) => {
        const current = prev.get(channelId);
        if (!current) return prev;
        const newMuted = !current.isMuted;
        effectiveVolume = newMuted ? 0 : current.volume;
        return withPeerChannelPatch(prev, channelId, { isMuted: newMuted });
      });
      if (effectiveVolume !== null) {
        streamingSetPeerVolume(Math.round(effectiveVolume * 2)).catch(console.error);
      }
    }
  }, [localChannelState.isMuted]);

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
  const mixerChannels = useMemo((): Channel[] => {
    const channels: Channel[] = [];

    // Local (self) channel
    channels.push({
      id: "local",
      name: peerName,
      type: "local",
      sampleRate: localSampleRate,
      channelCount: localChannelCount,
      levelL: localChannelState.isMuted ? 0 : inputLevel,
      levelR: localChannelState.isMuted ? 0 : inputLevel, // Mono input shown as dual
      volume: localChannelState.volume,
      pan: localChannelState.pan,
      isMuted: localChannelState.isMuted,
      isMonitoring,
    });

    // Peer channels
    if (sessionState.status === "connected") {
      sessionState.participants.forEach((peer) => {
        const peerState = peerChannelStates.get(peer.id) || {
          volume: 80,
          pan: 0,
          isMuted: false,
        };
        channels.push({
          id: peer.id,
          name: peer.name,
          type: "remote",
          sampleRate: peerAudio?.sample_rate ?? 48000,
          channelCount: peerAudio?.channel_count ?? 2,
          levelL: peerState.isMuted ? 0 : outputLevel,
          levelR: peerState.isMuted ? 0 : outputLevel,
          volume: peerState.volume,
          pan: peerState.pan,
          isMuted: peerState.isMuted,
        });
      });
    }

    return channels;
  }, [localChannelState, isMonitoring, peerName, localSampleRate, localChannelCount, inputLevel, sessionState, peerChannelStates, peerAudio, outputLevel]);

  // Initialize peer channel states when participants change, and prune any
  // state left behind for a peer who has since left.
  useEffect(() => {
    if (sessionState.status !== "connected") return;
    const currentIds = new Set(sessionState.participants.map((p) => p.id));
    setPeerChannelStates((prevStates) => {
      const newStates = new Map(prevStates);
      for (const key of newStates.keys()) {
        if (!currentIds.has(key)) newStates.delete(key);
      }
      sessionState.participants.forEach((peer) => {
        if (!newStates.has(peer.id)) {
          newStates.set(peer.id, {
            volume: 80,
            pan: 0,
            isMuted: false,
          });
        }
      });
      return newStates;
    });
  }, [sessionState]);

  // Map SessionState to ConnectionPanel state
  const getConnectionPanelState = (): ConnectionState => {
    switch (sessionState.status) {
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

  // Get error message for ConnectionPanel
  const getErrorMessage = (): string | undefined => {
    if (sessionState.status === "error") {
      const formatted = formatErrorForDisplay(sessionState.message, t);
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

  // Handle cancel during connection
  const handleCancelConnection = useCallback(async () => {
    if (connectionId !== null) {
      try {
        await signalingDisconnect(connectionId);
      } catch (e) {
        console.error("Failed to disconnect:", e);
      }
      setConnectionId(null);
    }
    setSessionState({ status: "idle" });
    // Re-attempt connection to server
    await autoConnect();
  }, [connectionId]);

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
          rawErrorMessage={sessionState.status === "error" ? sessionState.message : undefined}
          serverUrl={serverUrl}
          connected={connectionId !== null}
          notConnectedReason={t("session.notConnected.reason", "Not connected to the server yet")}
          onCreateRoom={handleCreateRoom}
          onJoinRoom={(code) => handleJoinRoom(code)}
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
            sessionState.status === "connecting_server"
              ? t("signaling.connecting", "Connecting to server...")
              : sessionState.status === "creating"
                ? t("session.create.loading")
                : t("session.join.loading")
          }
          cancelText={t("common.button.cancel", "Cancel")}
          historyTitle={t("connectionHistory.title")}
          testRoomCode={testRoomCode ?? undefined}
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
    if (sessionState.status !== "connected") return null;

    // The invite code as the server reported it. No fallback derived from the
    // room's UUID: those six characters look exactly like an invite code but
    // no room can be joined with them, so a participant who copied it could
    // not invite anyone (ADR-026).
    const roomCode = currentInviteCode;
    const participantCount = sessionState.participants.length + 1;
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
                {sessionState.participants.map((participant) => (
                  <li key={participant.id} className="participant">
                    <span className="participant__info">
                      <span className="participant__icon">
                        <UserIcon />
                      </span>
                      <span className="participant__name">{participant.name}</span>
                    </span>
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

            <button
              type="button"
              className="room-sidebar__leave"
              data-testid="leave-room"
              onClick={() => setShowLeaveDialog(true)}
            >
              <span>{t("session.leave.button")}</span>
              <LogOutIcon />
            </button>
          </aside>

          {/* Mixer */}
          <div className="main-mixer-column">
            <MixerPanel
              channels={mixerChannels}
              onChannelVolumeChange={handleChannelVolumeChange}
              onChannelPanChange={handleChannelPanChange}
              onChannelMuteToggle={handleChannelMuteToggle}
              onChannelMonitorToggle={handleChannelMonitorToggle}
            />
          </div>

          {/* Chat (docked column) */}
          <div className="main-chat-column">
            <ChatPanelAdapter connId={connectionId} myPeerId={myPeerId} />
          </div>
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
          {signalingReconnectState === "reconnecting" && (
            <div className="main-footer__warning">
              <Toast type="warning" message={t("session.signalingReconnect.inProgress")} />
            </div>
          )}
          {signalingReconnectState === "failed" && (
            <div className="main-footer__warning">
              <Toast
                type="error"
                message={t("session.signalingReconnect.failed", {
                  reason: signalingReconnectError ?? "",
                })}
              />
              <button
                type="button"
                className="main-footer__reconnect"
                onClick={() => attemptSignalingReconnect(currentInviteCode)}
              >
                {t("session.reconnect.retry")}
              </button>
            </div>
          )}
        </footer>

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
  const showConnectionPanel = sessionState.status !== "connected";

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
