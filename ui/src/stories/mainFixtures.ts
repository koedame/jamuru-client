/**
 * What a backend says about a session, for the stories that draw MainScreen
 * (the main window, and the helper's window that draws the helped app's one).
 */
import type { MixerSnapshot, PeerLinkStatus, SessionSnapshot } from "../lib/tauri";
import type { Answers } from "./fakeBackend";

export function snapshot(changes: Partial<SessionSnapshot> = {}): SessionSnapshot {
  return {
    revision: 1,
    phase: "server_connected",
    connection_id: 1,
    test_room_invite_code: null,
    joining_code: null,
    error: null,
    room: null,
    signaling_reconnect: "idle",
    signaling_reconnect_error: null,
    streaming_peer_ids: [],
    streaming_peer_id: null,
    ...changes,
  };
}

export function inRoom(names: string[], changes: Partial<SessionSnapshot> = {}): SessionSnapshot {
  const ids = names.map((_, i) => `peer-${i + 1}`);
  return snapshot({
    phase: "connected",
    room: {
      room_id: "room-1",
      invite_code: "ABC234XYZ",
      peer_id: "me",
      peer_name: "Yuki",
      participants: names.map((name, i) => ({ id: ids[i], name, features: ["peer_message"] })),
    },
    streaming_peer_ids: ids,
    streaming_peer_id: ids[0] ?? null,
    ...changes,
  });
}

export function link(peerId: string, up: number, down: number, level: number): PeerLinkStatus {
  return {
    peer_id: peerId,
    connection_state: "connected",
    connection_error: null,
    network: {
      rtt_ms: up + down,
      jitter_ms: 1,
      packet_loss_percent: 0,
      quality: "good",
      measured_bps: 1_200_000,
      required_bps: 1_000_000,
      bandwidth_status: "sufficient",
      uptime_seconds: 754,
      packets_sent: 90000,
      packets_received: 89990,
      encryption: "encrypted",
      bytes_sent: 1_000_000,
      bytes_received: 1_000_000,
    },
    latency: {
      upstream: [],
      upstream_total_ms: up,
      downstream: [],
      downstream_total_ms: down,
      roundtrip_total_ms: up + down,
    },
    peer_audio: null,
    level,
    buffer: { own: { mode: "preset" }, delay_ms10: 0, granted: null },
    dropout: null,
    unstable: false,
  } as PeerLinkStatus;
}

export const idleRecording = { recording: false, seconds: 0, dropped_frames: 0, file_name: null, last: null, peers: [] };

export function answers(session: SessionSnapshot, extra: Answers = {}): Answers {
  const peers = session.room?.participants ?? [];
  const mixer: MixerSnapshot = {
    revision: 1,
    local: { volume: 80, pan: 0 },
    peers: Object.fromEntries(peers.map((p, i) => [p.id, { volume: 75 - i * 5, pan: i % 2 ? 30 : -30, muted: false }])),
  };
  const links = peers.map((p, i) => link(p.id, 8 + i * 6, 9 + i * 6, 40 + i * 15));
  return {
    session_get: session,
    mixer_get: mixer,
    config_load: { peer_name: "Yuki", ask_name_on_entry: false },
    config_get_sample_rate: 48000,
    config_get_transmit_channels: 2,
    config_get_effective_server_url: "signal.example.org",
    update_notice_get: null,
    signaling_get_chat_messages: [],
    recording_status: idleRecording,
    streaming_status:
      links.length > 0
        ? {
            is_active: true,
            peers: links,
            device_problems: [],
            wasapi_opened: { input: null, output: null },
            network: links[0].network,
            latency: links[0].latency,
            audio_quality: null,
            peer_audio: null,
            connection_state: "connected",
            connection_error: null,
            is_muted: false,
            is_monitoring: false,
            input_level: 35,
            output_level: 40,
          }
        : { is_active: false, peers: [], device_problems: [], wasapi_opened: { input: null, output: null } },
    streaming_get_mute: false,
    streaming_get_input_level: 35,
    ...extra,
  };
}
