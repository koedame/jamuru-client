/**
 * MainScreen: what happens to the audio when the peer we are streaming to
 * leaves the room.
 *
 * The bug this guards: after B left, the app kept its audio session to B
 * (which then reported the connection as lost) and, because it still counted
 * as "streaming", never started audio to the next person who joined.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';

import { MainScreen } from './MainScreen';
import type { PeerInfo, SignalingEvent } from '../lib/tauri';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
  emit: vi.fn(),
  emitTo: vi.fn(),
}));
vi.mock('@tauri-apps/plugin-deep-link', () => ({
  getCurrent: vi.fn(() => Promise.resolve(null)),
  onOpenUrl: vi.fn(() => Promise.resolve(() => {})),
}));

function peer(id: string, addr: string | null): PeerInfo {
  return {
    id,
    name: id,
    candidates: addr ? [{ address: addr, candidate_type: 'Host', priority: 100 }] : [],
    public_addr: null,
    local_addr: null,
  };
}

/** Commands the app sent to the backend, in order, with their arguments. */
let calls: Array<{ cmd: string; args: Record<string, unknown> | undefined }>;
/** Events the signaling server has queued for the app's next poll. */
let queuedEvents: SignalingEvent[];
/** Peers already in the room when the app creates it. */
let peersAtCreate: PeerInfo[];

function commandNames(): string[] {
  return calls.map((c) => c.cmd);
}

function streamingStartTargets(): unknown[] {
  return calls.filter((c) => c.cmd === 'streaming_start').map((c) => c.args?.remoteAddr);
}

function callCount(cmd: string): number {
  return calls.filter((c) => c.cmd === cmd).length;
}

const POLL_WAIT = { timeout: 3000 };

async function enterRoom() {
  render(<MainScreen />);
  fireEvent.click(await screen.findByTestId('connection-panel-create-room'));
  await waitFor(() => expect(callCount('signaling_publish_local_candidates')).toBe(1), POLL_WAIT);
}

beforeEach(() => {
  calls = [];
  queuedEvents = [];
  peersAtCreate = [];
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    calls.push({ cmd, args });
    switch (cmd) {
      case 'signaling_connect':
        return 1;
      case 'signaling_list_rooms':
        return [];
      case 'signaling_create_room':
        return { room_id: 'room-1', peer_id: 'me', invite_code: 'ABC123', peers: peersAtCreate };
      case 'signaling_poll_events': {
        const events = queuedEvents;
        queuedEvents = [];
        return events;
      }
      case 'streaming_prepare':
        return '0.0.0.0:40000';
      case 'streaming_status':
        return { is_active: false };
      case 'audio_get_current_devices':
        return { input_device_id: null, output_device_id: null };
      case 'audio_get_buffer_size':
        return 64;
      case 'config_get_peer_name':
        return 'Me';
      case 'config_get_connection_history':
        return [];
      case 'config_get_sample_rate':
        return 48000;
      case 'config_get_transmit_channels':
        return 1;
      case 'config_get_effective_server_url':
        return 'test-server';
      case 'signaling_get_chat_messages':
        return [];
      default:
        return undefined;
    }
  });
});

describe('MainScreen when the peer being streamed to leaves', () => {
  it('その相手に音声を流しているとき、退室を受けたらストリーミングを止めること', async () => {
    peersAtCreate = [peer('b', '192.0.2.2:5000')];
    await enterRoom();
    await waitFor(() => expect(streamingStartTargets()).toEqual(['192.0.2.2:5000']), POLL_WAIT);

    queuedEvents.push({ type: 'PeerLeft', peer_id: 'b' });

    await waitFor(() => expect(callCount('streaming_stop')).toBe(1), POLL_WAIT);
  });

  it('退室のあとに次の人がアドレスを公開したとき、その人に音声を流し始めること', async () => {
    peersAtCreate = [peer('b', '192.0.2.2:5000')];
    await enterRoom();
    await waitFor(() => expect(streamingStartTargets()).toHaveLength(1), POLL_WAIT);

    queuedEvents.push({ type: 'PeerLeft', peer_id: 'b' });
    await waitFor(() => expect(callCount('streaming_stop')).toBe(1), POLL_WAIT);

    queuedEvents.push({ type: 'PeerJoined', peer: peer('c', null) });
    queuedEvents.push({ type: 'PeerUpdated', peer: peer('c', '192.0.2.3:6000') });

    await waitFor(
      () => expect(streamingStartTargets()).toEqual(['192.0.2.2:5000', '192.0.2.3:6000']),
      POLL_WAIT
    );
  });

  it('退室した時点で次の人がすでにアドレスを持っているとき、自分のアドレスを公開し直してその人へ繋ぎ直すこと', async () => {
    peersAtCreate = [peer('b', '192.0.2.2:5000')];
    await enterRoom();
    await waitFor(() => expect(streamingStartTargets()).toHaveLength(1), POLL_WAIT);

    // C joined while A and B were talking; A did not stream to C then.
    queuedEvents.push({ type: 'PeerJoined', peer: peer('c', '192.0.2.3:6000') });
    queuedEvents.push({ type: 'PeerLeft', peer_id: 'b' });

    await waitFor(
      () => expect(streamingStartTargets()).toEqual(['192.0.2.2:5000', '192.0.2.3:6000']),
      POLL_WAIT
    );
    // Starting audio consumes the advertised socket, so the address has to be
    // advertised again (a fresh port) before anyone can send to us.
    const afterLeave = commandNames().slice(commandNames().indexOf('streaming_stop'));
    expect(afterLeave.slice(0, 3)).toEqual([
      'streaming_stop',
      'streaming_prepare',
      'signaling_publish_local_candidates',
    ]);
  });

  it('音声を流していない相手が退室したとき、ストリーミングを止めないこと', async () => {
    peersAtCreate = [peer('b', '192.0.2.2:5000')];
    await enterRoom();
    await waitFor(() => expect(streamingStartTargets()).toHaveLength(1), POLL_WAIT);

    queuedEvents.push({ type: 'PeerJoined', peer: peer('c', '192.0.2.3:6000') });
    queuedEvents.push({ type: 'PeerLeft', peer_id: 'c' });
    // Let the events be polled and handled before asserting nothing happened.
    await waitFor(() => expect(queuedEvents).toHaveLength(0), POLL_WAIT);
    await new Promise((resolve) => setTimeout(resolve, 700));

    expect(callCount('streaming_stop')).toBe(0);
    expect(streamingStartTargets()).toEqual(['192.0.2.2:5000']);
  });
});
