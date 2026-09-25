/**
 * Tauri IPC wrapper functions
 *
 * Provides typed interfaces for communicating with the Rust backend.
 */

import { invoke } from "./invoke";

/**
 * Room information from signaling server
 */
export interface RoomInfo {
  id: string;
  name: string;
  peer_count: number;
  max_peers: number;
  has_password: boolean;
  invite_code: string;
  /** The room the server offers for trying a connection (absent from older servers) */
  test_room?: boolean;
}

/**
 * A single address candidate for connection (host/LAN or server reflexive/public)
 */
export interface AddressCandidate {
  address: string;
  candidate_type: "Host" | "ServerReflexive";
  priority: number;
}

/**
 * Peer information
 */
export interface PeerInfo {
  id: string;
  name: string;
  candidates: AddressCandidate[];
  public_addr: string | null;
  local_addr: string | null;
}

/**
 * All addresses a peer can be reached at, highest priority first: its
 * gathered candidates (LAN/host ranked above public/server-reflexive), then
 * its legacy `public_addr`/`local_addr` if not already among them.
 *
 * Mirrors `PeerInfo::get_sorted_candidates` (src/network/signaling.rs) so a
 * peer on the same network is tried directly instead of only through its
 * public address (REQ-CON-113).
 */
export function peerSortedAddrs(peer: PeerInfo): string[] {
  const sorted = [...peer.candidates].sort((a, b) => b.priority - a.priority);
  const addrs = sorted.map((c) => c.address);
  for (const addr of [peer.public_addr, peer.local_addr]) {
    if (addr && !addrs.includes(addr)) addrs.push(addr);
  }
  return addrs;
}

/**
 * Result of joining or creating a room
 */
export interface JoinResult {
  room_id: string;
  peer_id: string;
  invite_code: string;
  peers: PeerInfo[];
}

/**
 * Reaction on a chat message
 */
export interface Reaction {
  /** The emoji used for this reaction */
  emoji: string;
  /** List of user IDs who reacted with this emoji */
  user_ids: string[];
  /** Total count of this reaction */
  count: number;
}

/**
 * Chat message from signaling
 */
export interface ChatMessage {
  id: string;
  sender_id: string;
  sender_name: string;
  content: string;
  timestamp: number;
  is_system: boolean;
  /**
   * "join"/"leave" for a system message representing that room event, else null.
   * System messages carry no text (`content` is empty): the UI renders the
   * sentence from this and `sender_name`, so it follows the UI language.
   */
  system_kind: "join" | "leave" | null;
  /** Reactions on this message */
  reactions?: Reaction[];
}

/**
 * Signaling event types
 */
export type SignalingEvent =
  | { type: "PeerJoined"; peer: PeerInfo }
  | { type: "PeerLeft"; peer_id: string }
  | { type: "PeerUpdated"; peer: PeerInfo }
  | { type: "ChatMessageReceived"; message: ChatMessage }
  | { type: "RoomClosed"; reason: string }
  | { type: "Kicked"; peer_id: string; reason: string }
  | { type: "ConnectionLost"; reason: string };

/**
 * Connect to the signaling server. The backend picks the server: the one in
 * the config file, otherwise the build's default (ADR-030)
 * @returns Connection ID for subsequent operations
 */
export async function signalingConnect(): Promise<number> {
  return invoke("signaling_connect");
}

/**
 * Disconnect from a signaling server
 * @param connId Connection ID from signalingConnect
 */
export async function signalingDisconnect(connId: number): Promise<void> {
  return invoke("signaling_disconnect", { connId });
}

/**
 * List available rooms on the signaling server
 * @param connId Connection ID from signalingConnect
 * @returns Array of room information
 */
export async function signalingListRooms(connId: number): Promise<RoomInfo[]> {
  return invoke("signaling_list_rooms", { connId });
}

/**
 * Join an existing room
 * @param connId Connection ID from signalingConnect
 * @param roomId Room ID to join
 * @param peerName Display name for this peer
 * @returns Join result with room info and peer list
 */
export async function signalingJoinRoom(
  connId: number,
  roomId: string,
  peerName: string
): Promise<JoinResult> {
  return invoke("signaling_join_room", { connId, roomId, peerName });
}

/**
 * Leave the current room
 * @param connId Connection ID from signalingConnect
 */
export async function signalingLeaveRoom(connId: number): Promise<void> {
  return invoke("signaling_leave_room", { connId });
}

/**
 * Create a new room
 * @param connId Connection ID from signalingConnect
 * @param roomName Name for the new room
 * @param peerName Display name for this peer (room creator)
 * @returns Join result with the new room info
 */
export async function signalingCreateRoom(
  connId: number,
  roomName: string,
  peerName: string
): Promise<JoinResult> {
  return invoke("signaling_create_room", { connId, roomName, peerName });
}

/**
 * Poll for signaling events (peer join/leave, chat messages)
 * @param connId Connection ID from signalingConnect
 * @returns Array of signaling events
 */
export async function signalingPollEvents(
  connId: number
): Promise<SignalingEvent[]> {
  return invoke("signaling_poll_events", { connId });
}

/**
 * Add a reaction to a chat message
 * @param messageId ID of the message to react to
 * @param emoji Emoji to add as reaction
 * @returns Updated chat message
 */
export async function signalingAddReaction(
  messageId: string,
  emoji: string
): Promise<ChatMessage> {
  return invoke("signaling_add_reaction", { messageId, emoji });
}

/**
 * Remove a reaction from a chat message
 * @param messageId ID of the message
 * @param emoji Emoji to remove
 * @returns Updated chat message
 */
export async function signalingRemoveReaction(
  messageId: string,
  emoji: string
): Promise<ChatMessage> {
  return invoke("signaling_remove_reaction", { messageId, emoji });
}

/**
 * Toggle a reaction on a chat message
 * If the user already reacted with this emoji, removes it.
 * Otherwise, adds the reaction.
 * @param messageId ID of the message
 * @param emoji Emoji to toggle
 * @returns Updated chat message
 */
export async function signalingToggleReaction(
  messageId: string,
  emoji: string
): Promise<ChatMessage> {
  return invoke("signaling_toggle_reaction", { messageId, emoji });
}

// ============================================================================
// Audio Device API
// ============================================================================

/**
 * Audio device information
 */
export interface AudioDeviceInfo {
  id: string;
  name: string;
  supported_sample_rates: number[];
  supported_channels: number[];
  is_default: boolean;
  is_asio: boolean;
}

/**
 * Current device selection
 */
export interface CurrentDevices {
  input_device_id: string | null;
  output_device_id: string | null;
}

/**
 * Get current device selection
 * @returns Current input and output device IDs
 */
export async function audioGetCurrentDevices(): Promise<CurrentDevices> {
  return invoke("audio_get_current_devices");
}

/**
 * Get the buffer size in effect (frame_size in samples, as saved)
 * @returns Buffer size (one of AudioSettings.buffer_sizes)
 */
export async function audioGetBufferSize(): Promise<number> {
  return invoke("audio_get_buffer_size");
}

// ============================================================================
// Audio Settings API (ADR-043)
// ============================================================================

/**
 * A left/right pair of 1-based device channels
 */
export interface ChannelPair {
  left: number;
  /** null for mono */
  right: number | null;
}

/**
 * The audio settings in effect, with the choices on offer.
 * Also the payload of the `audio:config-changed` event every window hears
 * after a change, whoever made it.
 */
export interface AudioSettings {
  input_devices: AudioDeviceInfo[];
  output_devices: AudioDeviceInfo[];
  /** The chosen input device; null means the system default */
  input_device_id: string | null;
  /** The chosen output device; null means the system default */
  output_device_id: string | null;
  input_channels: ChannelPair;
  output_channels: ChannelPair;
  transmit_channels: number;
  buffer_size: number;
  buffer_sizes: number[];
  sample_rate: number;
  sample_rates: SampleRateInfo[];
}

/**
 * One change to the audio settings, named by its setting
 */
export type SettingChange =
  | { setting: "input_device"; device_id: string }
  | { setting: "output_device"; device_id: string }
  | { setting: "input_channels"; left: number; right: number | null }
  | { setting: "output_channels"; left: number; right: number | null }
  | { setting: "transmit_channels"; count: number }
  | { setting: "buffer_size"; samples: number }
  | { setting: "sample_rate"; hz: number }
  | { setting: "preset"; preset: AudioPresetId };

/** Event every window hears after an audio setting changed */
export const AUDIO_SETTINGS_CHANGED = "audio:config-changed";

/**
 * Get the audio settings in effect, with the devices and values on offer
 */
export async function settingsGet(): Promise<AudioSettings> {
  return invoke("settings_get");
}

/**
 * Change one audio setting. The saved config and a running session both
 * follow, and every window hears about it.
 * @returns The settings now in effect
 */
export async function settingsChange(change: SettingChange): Promise<AudioSettings> {
  return invoke("settings_change", { change });
}

// ============================================================================
// Streaming API
// ============================================================================

/**
 * Connection quality band as classified by the core library
 *
 * good: RTT < 30ms and loss < 1%
 * fair: RTT < 100ms and loss < 5%
 * poor: RTT >= 100ms or loss >= 5%
 */
export type ConnectionQualityBand = 'good' | 'fair' | 'poor';

/**
 * Whether the measured link carries what the preset needs
 *
 * sufficient: at least 20% more than required
 * marginal: enough, but under 20% of headroom
 * insufficient: less than required
 * no_signal: the interval measured zero bytes - nothing is arriving from the
 * peer at all, which is not the same as a narrow link (REQ-LAT-127)
 */
export type BandwidthStatus = 'sufficient' | 'marginal' | 'insufficient' | 'no_signal';

/**
 * Network statistics
 */
export interface NetworkStats {
  /** Round-trip time in milliseconds, or null before the first sample arrives (REQ-LAT-130) */
  rtt_ms: number | null;
  /** Jitter in milliseconds */
  jitter_ms: number;
  /** Packet loss percentage (0-100) */
  packet_loss_percent: number;
  /**
   * Connection quality band, classified in the core library (REQ-LAT-121),
   * or null before the first RTT sample arrives (REQ-LAT-130).
   *
   * Do not re-derive this from rtt_ms and packet_loss_percent: the thresholds
   * are defined once, in `src/network/quality.rs`.
   */
  quality: ConnectionQualityBand | null;
  /** Measured throughput in bits per second, 0 before the first measurement */
  measured_bps: number;
  /** Bits per second the current preset needs in one direction */
  required_bps: number;
  /**
   * Whether the link carries what the preset needs, or null before measurement.
   *
   * A warning signal only: bitrate adaptation needs a variable-rate codec, and
   * no preset uses one (ADR-021).
   */
  bandwidth_status: BandwidthStatus | null;
  /** Connection uptime in seconds */
  uptime_seconds: number;
  /** Total packets sent */
  packets_sent: number;
  /** Total packets received */
  packets_received: number;
  /** Total bytes sent */
  bytes_sent: number;
  /** Total bytes received */
  bytes_received: number;
}

/**
 * Latency component breakdown
 */
export interface LatencyComponent {
  /** Component name */
  name: string;
  /** Latency in milliseconds */
  ms: number;
  /** Additional info (e.g., "128 samples @ 48000 Hz") */
  info: string | null;
}

/**
 * Detailed latency breakdown
 */
export interface DetailedLatency {
  /** Upstream components (self -> peer) */
  upstream: LatencyComponent[];
  /** Upstream total in ms */
  upstream_total_ms: number;
  /** Downstream components (peer -> self) */
  downstream: LatencyComponent[];
  /** Downstream total in ms */
  downstream_total_ms: number;
  /** Round-trip total in ms */
  roundtrip_total_ms: number;
}

/**
 * Audio quality metrics
 */
export interface AudioQuality {
  /** Number of buffer underruns (audio glitches due to CPU/scheduling) */
  underrun_count: number;
  /** Times the play-out delay was adjusted automatically (REQ-LAT-108) */
  delay_adjustments: number;
}

/**
 * Peer audio configuration (ADR-013)
 */
export interface PeerAudioInfo {
  /** Peer's sample rate in Hz */
  sample_rate: number;
  /** Peer's frame size in samples */
  frame_size: number;
  /** Peer's codec name */
  codec: string;
  /** Whether resampling is needed (peer sample rate != local sample rate) */
  needs_resampling: boolean;
  /** Peer's transmit channel count (1 = mono, 2 = stereo) */
  channel_count: number;
}

/**
 * Streaming status information
 */
export interface StreamingStatus {
  is_active: boolean;
  remote_addr: string | null;
  /** Whether microphone is muted */
  is_muted: boolean;
  /** Whether the user hears their own input directly */
  is_monitoring: boolean;
  /** Current input audio level (0-100) */
  input_level: number;
  /** Current output audio level (0-100, for master meter) */
  output_level: number;
  /** Network statistics */
  network: NetworkStats | null;
  /** Detailed latency breakdown */
  latency: DetailedLatency | null;
  /** Audio quality metrics */
  audio_quality: AudioQuality | null;
  /** Peer's audio configuration (ADR-013) */
  peer_audio: PeerAudioInfo | null;
  /**
   * Connection state (ADR-022). "connected" | "reconnecting" | "failed" | ...
   *
   * The UI shows reconnection progress from this and asks whether to retry once
   * it reads "failed".
   */
  connection_state: string | null;
  /** Why the connection failed, when connection_state is "failed" */
  connection_error: string | null;
}

/**
 * Start audio streaming to a remote peer
 * @param remoteAddr Remote address in format "ip:port"
 * @param inputDeviceId Optional input device ID
 * @param outputDeviceId Optional output device ID
 * @param bufferSize Buffer size in samples (32, 64, 128, or 256). Default: 64
 * @param sampleRate Sample rate in Hz (44100, 48000, or 96000). Default: 48000 (ADR-013)
 */
/**
 * Bind the audio socket and get the address peers should send to.
 *
 * Called on entering a room, before any peer address is known. Idempotent -
 * repeated calls return the same address (ADR-026).
 *
 * @returns The local audio address as "0.0.0.0:port"
 */
export async function streamingPrepare(): Promise<string> {
  return invoke("streaming_prepare");
}

/**
 * Advertise this app's audio address to everyone in the room.
 *
 * Two GUI instances cannot start streaming to each other without this: each
 * waits to see a peer address and neither publishes one (ADR-026).
 *
 * @param connId Signaling connection id
 * @param localPort Port from streamingPrepare
 * @returns How many address candidates were published
 */
export async function signalingPublishLocalCandidates(
  connId: number,
  localPort: number
): Promise<number> {
  return invoke("signaling_publish_local_candidates", { connId, localPort });
}

export async function streamingStart(
  remoteAddr: string,
  remoteCandidates?: string[],
  inputDeviceId?: string,
  outputDeviceId?: string,
  bufferSize?: number,
  sampleRate?: number
): Promise<void> {
  return invoke("streaming_start", {
    remoteAddr,
    remoteCandidates: remoteCandidates ?? null,
    inputDeviceId: inputDeviceId ?? null,
    outputDeviceId: outputDeviceId ?? null,
    bufferSize: bufferSize ?? 64,
    sampleRate: sampleRate ?? null,
  });
}

/**
 * Stop audio streaming
 */
export async function streamingStop(): Promise<void> {
  return invoke("streaming_stop");
}

/**
 * Retry the connection after automatic recovery gave up (REQ-CON-110)
 *
 * Backs the "reconnect?" prompt shown when `connection_state` reads "failed".
 */
export async function streamingReconnect(): Promise<void> {
  return invoke("streaming_reconnect");
}

/**
 * Get streaming status
 * @returns Current streaming status
 */
export async function streamingStatus(): Promise<StreamingStatus> {
  return invoke("streaming_status");
}

/**
 * Set mute state
 * @param muted Whether to mute the microphone
 */
export async function streamingSetMute(muted: boolean): Promise<void> {
  return invoke("streaming_set_mute", { muted });
}

/**
 * Switch local monitoring: hearing your own input directly, without the
 * network delay
 * @param enabled Whether to monitor
 */
export async function streamingSetMonitoring(enabled: boolean): Promise<void> {
  return invoke("streaming_set_monitoring", { enabled });
}

/**
 * Get mute state
 * @returns Whether the microphone is muted
 */
export async function streamingGetMute(): Promise<boolean> {
  return invoke("streaming_get_mute");
}

/**
 * Get current input audio level
 * @returns Audio level from 0 to 100
 */
export async function streamingGetInputLevel(): Promise<number> {
  return invoke("streaming_get_input_level");
}

/**
 * Set peer (received audio) volume
 * @param volume Volume from 0 to 200 (100 = unity gain, 200 = 2x)
 */
export async function streamingSetPeerVolume(volume: number): Promise<void> {
  return invoke("streaming_set_peer_volume", { volume: Math.round(volume) });
}

/**
 * Get peer volume
 * @returns Volume from 0 to 200 (100 = unity)
 */
export async function streamingGetPeerVolume(): Promise<number> {
  return invoke("streaming_get_peer_volume");
}

/**
 * Set peer (received audio) pan
 * @param pan Pan from -100 (full left) to 100 (full right), 0 = center
 */
export async function streamingSetPeerPan(pan: number): Promise<void> {
  return invoke("streaming_set_peer_pan", { pan: Math.round(pan) });
}

/**
 * Get peer pan
 * @returns Pan from -100 to 100 (0 = center)
 */
export async function streamingGetPeerPan(): Promise<number> {
  return invoke("streaming_get_peer_pan");
}

/**
 * Set local (microphone input) volume
 * @param volume Volume from 0 to 200 (100 = unity gain, 200 = 2x)
 */
export async function streamingSetLocalVolume(volume: number): Promise<void> {
  return invoke("streaming_set_local_volume", { volume: Math.round(volume) });
}

/**
 * Get local volume
 * @returns Volume from 0 to 200 (100 = unity)
 */
export async function streamingGetLocalVolume(): Promise<number> {
  return invoke("streaming_get_local_volume");
}

/**
 * Set local (microphone input) pan
 * @param pan Pan from -100 (full left) to 100 (full right), 0 = center
 */
export async function streamingSetLocalPan(pan: number): Promise<void> {
  return invoke("streaming_set_local_pan", { pan: Math.round(pan) });
}

/**
 * Get local pan
 * @returns Pan from -100 to 100 (0 = center)
 */
export async function streamingGetLocalPan(): Promise<number> {
  return invoke("streaming_get_local_pan");
}

// ============================================================================
// Configuration API
// ============================================================================

/**
 * Connection history entry
 */
export interface ConnectionHistoryEntry {
  /** Room code used for the connection */
  room_code: string;
  /** Timestamp of the connection (ISO 8601 format) */
  connected_at: string;
  /** Optional user-defined label for this connection */
  label: string | null;
}

/**
 * Application configuration
 */
export interface AppConfig {
  /** Selected input device ID (null = system default) */
  input_device_id: string | null;
  /** Selected output device ID (null = system default) */
  output_device_id: string | null;
  /** Audio buffer size in samples. Valid values: 32, 64, 128, 256 */
  buffer_size: number;
  /** Custom signaling server URL (null = use default server) */
  server_url: string | null;
  /** Selected audio preset */
  preset: AudioPresetId;
  /** Connection history (most recent first) */
  connection_history: ConnectionHistoryEntry[];
  /** Audio sample rate in Hz. Valid values: 44100, 48000, 96000 (ADR-013) */
  sample_rate: number;
  /** UI language (null = not chosen yet; use configGetLanguage/configSetLanguage) */
  language: string | null;
  /** Whether the app may tell the jamjam server how it runs (off unless the user turns it on) */
  usage_reporting: boolean;
  /** Whether the app installs a new release by itself (on unless the user turns it off in config.toml) */
  auto_update: boolean;
}

/**
 * Audio preset identifier
 */
export type AudioPresetId =
  | "zero-latency"
  | "ultra-low-latency"
  | "balanced"
  | "high-quality";

/**
 * Preset information
 */
export interface PresetInfo {
  /** Preset identifier (e.g., "zero-latency") */
  id: AudioPresetId;
  /** Recommended buffer size in samples */
  buffer_size: number;
  /** Recommended jitter buffer frames */
  jitter_buffer_frames: number;
}

/**
 * Load configuration from disk
 * @returns The current configuration (from file or defaults if file doesn't exist)
 */
export async function configLoad(): Promise<AppConfig> {
  return invoke("config_load");
}

/**
 * Save configuration to disk
 * @param config The configuration to save
 */
export async function configSave(config: AppConfig): Promise<void> {
  return invoke("config_save", { config });
}

/**
 * Get the signaling server URL from configuration
 * @returns The custom server URL, or null if using default
 */
export async function configGetServerUrl(): Promise<string | null> {
  return invoke("config_get_server_url");
}

/**
 * Set the signaling server URL in configuration
 * @param url The server URL, or null to use default
 */
export async function configSetServerUrl(url: string | null): Promise<void> {
  return invoke("config_set_server_url", { url });
}

/**
 * Get the signaling server URL the app will actually dial.
 * Unlike {@link configGetServerUrl}, resolves to the build default when no
 * override is configured, so it is never null.
 */
export async function configGetEffectiveServerUrl(): Promise<string> {
  return invoke("config_get_effective_server_url");
}

// ============================================================================
// Preset API
// ============================================================================

/**
 * List all available presets
 * @returns Array of preset information
 */
export async function configListPresets(): Promise<PresetInfo[]> {
  return invoke("config_list_presets");
}

/**
 * Get the current preset
 * @returns Current preset identifier
 */
export async function configGetPreset(): Promise<AudioPresetId> {
  return invoke("config_get_preset");
}

// ============================================================================
// Connection History API
// ============================================================================

/**
 * Get connection history
 * @returns List of past connections, most recent first
 */
export async function configGetConnectionHistory(): Promise<
  ConnectionHistoryEntry[]
> {
  return invoke("config_get_connection_history");
}

/**
 * Add a connection to history
 * @param roomCode Room code that was used
 * @param label Optional label for this connection
 */
export async function configAddConnectionHistory(
  roomCode: string,
  label?: string
): Promise<void> {
  return invoke("config_add_connection_history", {
    roomCode,
    label: label ?? null,
  });
}

/**
 * Remove a connection from history
 * @param roomCode Room code to remove
 */
export async function configRemoveConnectionHistory(
  roomCode: string
): Promise<void> {
  return invoke("config_remove_connection_history", { roomCode });
}

/**
 * Clear all connection history
 */
export async function configClearConnectionHistory(): Promise<void> {
  return invoke("config_clear_connection_history");
}

/**
 * Update connection history entry label
 * @param roomCode Room code to update
 * @param label New label (or null to remove label)
 */
export async function configUpdateConnectionHistoryLabel(
  roomCode: string,
  label: string | null
): Promise<void> {
  return invoke("config_update_connection_history_label", { roomCode, label });
}

/**
 * Get the user's display name
 * @returns The configured peer name
 */
export async function configGetPeerName(): Promise<string> {
  return invoke("config_get_peer_name");
}

/**
 * Set the user's display name
 * @param name New peer name (1-32 characters)
 */
export async function configSetPeerName(name: string): Promise<void> {
  return invoke("config_set_peer_name", { name });
}

// ============================================================================
// Sample Rate API (ADR-013)
// ============================================================================

/**
 * Sample rate information
 */
export interface SampleRateInfo {
  /** Sample rate in Hz */
  rate: number;
  /** Human-readable label */
  label: string;
  /** Whether this is the recommended rate (48000 Hz) */
  recommended: boolean;
}

/**
 * Get the configured sample rate
 * @returns Current sample rate in Hz
 */
export async function configGetSampleRate(): Promise<number> {
  return invoke("config_get_sample_rate");
}

// ============================================================================
// Channel Configuration API
// ============================================================================

/**
 * Get transmit channel count
 * @returns 1 for mono, 2 for stereo
 */
export async function configGetTransmitChannels(): Promise<number> {
  return invoke("config_get_transmit_channels");
}

/**
 * Get the persisted UI language
 * @returns "ja" | "en", or null if the user has not chosen one yet
 */
export async function configGetLanguage(): Promise<string | null> {
  return invoke("config_get_language");
}

/**
 * Set the UI language and notify every open window
 * @param language "ja" | "en"
 */
export async function configSetLanguage(language: string): Promise<void> {
  return invoke("config_set_language", { language });
}

// ============================================================================
// Diagnostics API
// ============================================================================

/**
 * Diagnostic grade for environment quality
 */
export type DiagnosticGrade = "A" | "B" | "C" | "Unknown";

/**
 * Problem severity levels
 */
export type ProblemSeverity = "Error" | "Warning" | "Info";

/**
 * Problem type, with any data needed to render its message. The UI builds
 * the localized message/suggestion text from this instead of receiving
 * pre-rendered English strings from the backend.
 */
export type ProblemCode =
  | { type: "NoConnectivity" }
  | { type: "NoIpv6" }
  | { type: "SymmetricNat" }
  | { type: "UnstableConnection" }
  | { type: "HighJitter"; data: { jitter_ms: number } }
  | { type: "SignalingUnreachable"; data: { url: string; error: string | null } }
  | { type: "InputEnumerationFailed"; data: { error: string } }
  | { type: "OutputEnumerationFailed"; data: { error: string } }
  | { type: "NoInputDevices" }
  | { type: "NoOutputDevices" }
  | { type: "InputNot48kHz"; data: { device_name: string } }
  | { type: "OutputNot48kHz"; data: { device_name: string } }
  | { type: "LowBufferUnsupported" }
  | { type: "NoAsioDevices" }
  | { type: "InsufficientRealtimeHeadroom" }
  | { type: "HighCpuUsage"; data: { usage_percent: number } }
  | { type: "LowMemory"; data: { available_mb: number } }
  | { type: "SmallBufferGlitchRisk" };

/**
 * Detected problem during diagnostics
 */
export interface DiagnosticProblem {
  /** Severity level */
  severity: ProblemSeverity;
  /** Category (network, audio, cpu) */
  category: string;
  /** Problem type, carrying the data needed to build its message */
  code: ProblemCode;
}

/**
 * Recommended preset based on diagnostics
 */
export type RecommendedPreset =
  | "ZeroLatency"
  | "UltraLowLatency"
  | "Balanced"
  | "HighQuality";

/**
 * IP support detection result
 */
export interface IpSupport {
  /** Whether IPv4 is available */
  ipv4_available: boolean;
  /** Whether IPv6 is available */
  ipv6_available: boolean;
  /** List of local IPv4 addresses */
  ipv4_addresses: string[];
  /** List of local IPv6 addresses */
  ipv6_addresses: string[];
  /** Public IPv4 address (from STUN) */
  public_ipv4: string | null;
  /** Public IPv6 address (from STUN) */
  public_ipv6: string | null;
}

/**
 * NAT type classification
 */
export type NatType =
  | "NoNat"
  | "FullCone"
  | "RestrictedCone"
  | "PortRestrictedCone"
  | "Symmetric"
  | "Unknown";

/**
 * Connection stability metrics
 */
export interface ConnectionStability {
  /** Average RTT to STUN servers (ms) */
  avg_rtt_ms: number | null;
  /** Minimum RTT observed (ms) */
  min_rtt_ms: number | null;
  /** Maximum RTT observed (ms) */
  max_rtt_ms: number | null;
  /** Number of successful probes */
  successful_probes: number;
  /** Number of failed probes */
  failed_probes: number;
  /** Estimated packet loss rate (0.0 - 1.0) */
  packet_loss_rate: number;
}

/**
 * Signaling server diagnostics
 */
export interface SignalingDiagnostics {
  /** Whether connection was successful */
  connected: boolean;
  /** Connection time (ms) */
  connection_time_ms: number | null;
  /** Error message if failed */
  error: string | null;
}

/**
 * Network diagnostics result
 */
export interface NetworkDiagnosticsResult {
  /** IP support detection */
  ip_support: IpSupport;
  /** Detected NAT type */
  nat_type: NatType;
  /** Connection stability grade */
  connection_stability: DiagnosticGrade;
  /** Measured jitter in milliseconds */
  jitter_ms: number | null;
  /** Stability metrics */
  stability_metrics: ConnectionStability;
  /** Signaling server diagnostics */
  signaling: SignalingDiagnostics;
  /** Detected problems */
  problems: DiagnosticProblem[];
}

/**
 * Audio device diagnostics
 */
export interface DeviceDiagnostics {
  /** Device ID */
  id: string;
  /** Device name */
  name: string;
  /** Whether this is the default device */
  is_default: boolean;
  /** Whether this is an ASIO device (Windows) */
  is_asio: boolean;
  /** Supported sample rates */
  supported_sample_rates: number[];
  /** Whether 48kHz is supported (required for jamjam) */
  supports_48khz: boolean;
  /** Supported channel counts */
  supported_channels: number[];
  /** Device grade */
  grade: DiagnosticGrade;
}

/**
 * Low-latency support detection
 */
export interface LowLatencySupport {
  /** Whether ASIO is available (Windows) */
  asio_available: boolean;
  /** List of available ASIO devices */
  asio_devices: string[];
  /** Whether 32-sample buffer is supported */
  supports_32_samples: boolean;
  /** Whether 64-sample buffer is supported */
  supports_64_samples: boolean;
  /** Whether 128-sample buffer is supported */
  supports_128_samples: boolean;
  /** Minimum supported buffer size */
  min_buffer_size: number | null;
  /** Estimated minimum latency at 48kHz (ms) */
  estimated_min_latency_ms: number | null;
}

/**
 * Where a diagnosed device came from: the device set in Settings, or the OS
 * default because Settings has nothing configured.
 */
export type DeviceSource = "Configured" | "OsDefault";

/**
 * Audio diagnostics result
 */
export interface AudioDiagnosticsResult {
  /** Available input devices */
  input_devices: DeviceDiagnostics[];
  /** Available output devices */
  output_devices: DeviceDiagnostics[];
  /** Diagnostics for the input device streaming would actually use */
  selected_input: DeviceDiagnostics | null;
  /** Diagnostics for the output device streaming would actually use */
  selected_output: DeviceDiagnostics | null;
  /** Whether `selected_input` is the configured device or the OS default */
  input_source: DeviceSource;
  /** Whether `selected_output` is the configured device or the OS default */
  output_source: DeviceSource;
  /** Low-latency support information */
  low_latency_support: LowLatencySupport;
  /** Overall audio grade */
  overall_grade: DiagnosticGrade;
  /** Detected problems */
  problems: DiagnosticProblem[];
}

/**
 * CPU benchmark result for a specific buffer size
 */
export interface CpuBenchmarkResult {
  /** Time to process one frame (microseconds) */
  processing_time_us: number;
  /** Frame duration at 48kHz (microseconds) */
  frame_duration_us: number;
  /** Real-time factor (< 1.0 = faster than real-time) */
  realtime_factor: number;
  /** Buffer size used for benchmark */
  buffer_size: number;
}

/**
 * System resource information
 */
export interface SystemResources {
  /** Number of CPU cores */
  cpu_cores: number;
  /** Current CPU usage (0.0 - 1.0), if available */
  cpu_usage: number | null;
  /** Available memory in MB, if available */
  available_memory_mb: number | null;
}

/**
 * CPU diagnostics result
 */
export interface CpuDiagnosticsResult {
  /** Benchmark results for different buffer sizes */
  benchmarks: CpuBenchmarkResult[];
  /** System resource information */
  system: SystemResources;
  /** Overall CPU grade */
  grade: DiagnosticGrade;
  /** Whether real-time processing is achievable */
  realtime_capable: boolean;
  /** Detected problems */
  problems: DiagnosticProblem[];
}

/**
 * Complete diagnostics result
 */
export interface CompleteDiagnosticsResult {
  /** Network diagnostics */
  network: NetworkDiagnosticsResult;
  /** Audio diagnostics */
  audio: AudioDiagnosticsResult;
  /** CPU diagnostics */
  cpu: CpuDiagnosticsResult;
  /** Overall score (0-100) */
  overall_score: number;
  /** Recommended preset based on diagnostics */
  recommended_preset: RecommendedPreset;
  /** Whether zero-latency mode is compatible */
  zero_latency_compatible: boolean;
  /** All detected problems across categories */
  problems: DiagnosticProblem[];
}

/**
 * Run complete diagnostics (network, audio, CPU)
 * @returns Complete diagnostics result
 */
export async function diagnosticsRunComplete(): Promise<CompleteDiagnosticsResult> {
  return invoke("diagnostics_run_complete");
}

/**
 * Run network diagnostics only
 * @returns Network diagnostics result
 */
export async function diagnosticsRunNetwork(): Promise<NetworkDiagnosticsResult> {
  return invoke("diagnostics_run_network");
}

/**
 * Run audio diagnostics only
 * @returns Audio diagnostics result
 */
export async function diagnosticsRunAudio(): Promise<AudioDiagnosticsResult> {
  return invoke("diagnostics_run_audio");
}

/**
 * Run CPU diagnostics only
 * @returns CPU diagnostics result
 */
export async function diagnosticsRunCpu(): Promise<CpuDiagnosticsResult> {
  return invoke("diagnostics_run_cpu");
}

/**
 * Get recommended preset based on current environment
 * @returns Recommended preset name
 */
export async function diagnosticsGetRecommendedPreset(): Promise<string> {
  return invoke("diagnostics_get_recommended_preset");
}

/**
 * Check if zero-latency mode is compatible with current environment
 * @returns Whether zero-latency mode is compatible
 */
export async function diagnosticsCheckZeroLatency(): Promise<boolean> {
  return invoke("diagnostics_check_zero_latency");
}

// ============================================================================
// Window Management API
// ============================================================================

/**
 * Window labels (matches Rust constants)
 */
export const WindowLabels = {
  CONNECTION: "connection",
  MIXER: "mixer",
  CHAT: "chat",
  SETTINGS: "settings",
} as const;

export type WindowLabel = (typeof WindowLabels)[keyof typeof WindowLabels];

/**
 * Open the settings window
 */
export async function windowOpenSettings(): Promise<void> {
  return invoke("window_open_settings");
}

/**
 * Close the settings window
 */
export async function windowCloseSettings(): Promise<void> {
  return invoke("window_close_settings");
}

/**
 * Toggle chat window visibility
 */
export async function windowToggleChat(): Promise<void> {
  return invoke("window_toggle_chat");
}

/**
 * Show chat window (must be in session)
 */
export async function windowShowChat(): Promise<void> {
  return invoke("window_show_chat");
}

/**
 * Hide chat window
 */
export async function windowHideChat(): Promise<void> {
  return invoke("window_hide_chat");
}

/**
 * Transition to connected state (opens mixer/chat windows, closes connection window)
 */
export async function windowSessionConnected(): Promise<void> {
  return invoke("window_session_connected");
}

/**
 * Transition to disconnected state (closes session windows, opens connection window)
 * @param reason Optional reason for disconnection
 */
export async function windowSessionDisconnected(
  reason?: string
): Promise<void> {
  return invoke("window_session_disconnected", { reason: reason ?? null });
}

/**
 * Check if currently in a session
 * @returns Whether the app is in a connected session
 */
export async function windowIsInSession(): Promise<boolean> {
  return invoke("window_is_in_session");
}

/**
 * Focus a specific window by label
 * @param label Window label to focus
 */
export async function windowFocus(label: WindowLabel): Promise<void> {
  return invoke("window_focus", { label });
}

/**
 * Resize the main window (e.g. compact for the join screen, wider once
 * connected) and update its enforced minimum size to match, so the window
 * can't be shrunk below what the current layout can render.
 * @param width Logical width in pixels
 * @param height Logical height in pixels
 * @param minWidth Logical minimum width in pixels
 * @param minHeight Logical minimum height in pixels
 */
export async function windowResizeMain(
  width: number,
  height: number,
  minWidth: number,
  minHeight: number
): Promise<void> {
  return invoke("window_resize_main", { width, height, minWidth, minHeight });
}

// =============================================================================
// Log file (ADR-036)
// =============================================================================

/**
 * Open the folder that holds `jamjam.log` in the OS file manager.
 *
 * @returns The folder's path, so it can be shown when the file manager cannot be opened
 */
export async function logOpenDir(): Promise<string> {
  return invoke("log_open_dir");
}

// =============================================================================
// Usage reporting (REQ-TEL)
// =============================================================================

/**
 * The lines the next usage report will contain, exactly as they would be sent
 * (one JSON object per line, the install ID included).
 *
 * @returns An empty string while usage reporting is off
 */
export async function usagePreview(): Promise<string> {
  return invoke("usage_preview");
}
