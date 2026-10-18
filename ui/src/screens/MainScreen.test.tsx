/**
 * MainScreen: draws the session the backend owns (ADR-044 §6).
 *
 * Which step the app is at, the room and who is in it, and the audio link are
 * the backend's - its own tests cover the rules (`src-tauri/src/session`).
 * What the screen does is draw the snapshot it reads and hears, and ask for
 * one operation at a time.
 */

import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import { act, render, screen, waitFor, fireEvent, within } from '@testing-library/react';

import i18n from '../i18n';
import { MainScreen } from './MainScreen';
import type { AudioEncryption, AudioQuality, DeviceProblem, MixerSnapshot, PeerLinkStatus, SessionSnapshot } from '../lib/tauri';

const invoke = vi.hoisted(() => vi.fn());
/** What the screen listens for, by event name. */
const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
/** The handler the deep-link plugin was given, for a link that arrives while running. */
const openUrl = vi.hoisted(() => ({ handler: null as null | ((urls: string[]) => void) }));

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return Promise.resolve(() => listeners.delete(name));
  }),
  emit: vi.fn(),
  emitTo: vi.fn(),
}));
vi.mock('@tauri-apps/plugin-deep-link', () => ({
  getCurrent: vi.fn(() => Promise.resolve(null)),
  onOpenUrl: vi.fn((handler: (urls: string[]) => void) => {
    openUrl.handler = handler;
    return Promise.resolve(() => {});
  }),
}));

function snapshot(changes: Partial<SessionSnapshot> = {}): SessionSnapshot {
  return {
    revision: 1,
    phase: 'server_connected',
    connection_id: 1,
    test_room_invite_code: null,
    joining_code: null,
    error: null,
    room: null,
    signaling_reconnect: 'idle',
    signaling_reconnect_error: null,
    streaming_peer_ids: [],
    streaming_peer_id: null,
    ...changes,
  };
}

function inRoom(changes: Partial<SessionSnapshot> = {}): SessionSnapshot {
  return snapshot({
    phase: 'connected',
    room: {
      room_id: 'room-1',
      invite_code: 'ABC234XYZ',
      peer_id: 'me',
      peer_name: 'Me',
      participants: [{ id: 'b', name: 'Aki', features: ['peer_message'] }],
    },
    ...changes,
  });
}

/** What the backend says the session is, for `session_get`. */
let current: SessionSnapshot | Promise<SessionSnapshot>;
/** Where the backend says the faders stand, for `mixer_get`. */
let mixerNow: MixerSnapshot;
/** The devices the backend says it could not open, for `streaming_status`. */
let statusDeviceProblems: DeviceProblem[];
/** Whether the backend says the audio to the peer is encrypted, for `streaming_status`; `null` while no session runs. */
let statusEncryption: AudioEncryption | null;
/** The audio links the backend says there are, for `streaming_status`; `null` while no session runs. */
let statusLinks: PeerLinkStatus[] | null;
/** The audio quality the backend says, for `streaming_status`. */
let statusAudioQuality: AudioQuality | null;

/** The audio link to participant `peerId`, in `state`, as `streaming_status` lists it */
function link(
  peerId: string,
  state: string,
  measured?: { up: number; down: number; level?: number; encryption?: AudioEncryption }
): PeerLinkStatus {
  return {
    peer_id: peerId,
    connection_state: state,
    connection_error: null,
    network: measured
      ? {
          rtt_ms: measured.up + measured.down,
          jitter_ms: 0,
          packet_loss_percent: 0,
          quality: 'good',
          measured_bps: 0,
          required_bps: 0,
          bandwidth_status: 'sufficient',
          uptime_seconds: 0,
          packets_sent: 0,
          packets_received: 10,
          encryption: measured.encryption ?? 'encrypted',
          peer_checked: true,
          bytes_sent: 0,
          bytes_received: 0,
        }
      : null,
    latency: measured
      ? {
          upstream: [],
          upstream_total_ms: measured.up,
          downstream: [],
          downstream_total_ms: measured.down,
          roundtrip_total_ms: measured.up + measured.down,
        }
      : null,
    peer_audio: null,
    level: measured?.level ?? 0,
  };
}

/** The room with Aki (b) and Ben (c) in it */
function roomOfThree(): SessionSnapshot {
  const room = inRoom().room!;
  return inRoom({
    room: {
      ...room,
      participants: [
        { id: 'b', name: 'Aki', features: [] },
        { id: 'c', name: 'Ben', features: [] },
      ],
    },
  });
}
/** What the backend says the saved config is, for `config_load`; `undefined` leaves the name unasked. */
let savedConfig: { peer_name: string; ask_name_on_entry: boolean } | undefined;
/** The newer release the backend says it found, for `update_notice_get`. */
let newerRelease: string | null;
/** Commands the screen sent to the backend, in order, with their arguments. */
let calls: Array<{ cmd: string; args: Record<string, unknown> | undefined }>;

function faders(changes: Partial<MixerSnapshot> = {}): MixerSnapshot {
  return {
    revision: 1,
    local: { volume: 80, pan: 0 },
    peers: { b: { volume: 80, pan: 0, muted: false } },
    ...changes,
  };
}

function callsTo(cmd: string) {
  return calls.filter((c) => c.cmd === cmd);
}

/** The backend announces a new state of the session. */
function announce(next: SessionSnapshot) {
  act(() => listeners.get('session:changed')!({ payload: next }));
}

beforeAll(async () => {
  await i18n.changeLanguage('en');
});

beforeEach(() => {
  calls = [];
  statusDeviceProblems = [];
  statusEncryption = null;
  statusLinks = null;
  statusAudioQuality = null;
  newerRelease = null;
  savedConfig = undefined;
  current = snapshot();
  mixerNow = faders();
  openUrl.handler = null;
  listeners.clear();
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    calls.push({ cmd, args });
    switch (cmd) {
      case 'session_get':
        return current;
      case 'mixer_get':
        return mixerNow;
      case 'update_notice_get':
        return newerRelease;
      case 'config_load':
        return savedConfig;
      case 'streaming_status': {
        const peers =
          statusLinks ??
          (statusEncryption
            ? [link('b', 'connected', { up: 5, down: 5, encryption: statusEncryption })]
            : null);
        if (peers) {
          return {
            is_active: true,
            peers,
            device_problems: statusDeviceProblems,
            network: peers[0]?.network ?? null,
            latency: peers[0]?.latency ?? null,
            audio_quality: statusAudioQuality,
            peer_audio: null,
            connection_state: peers[0]?.connection_state ?? null,
            connection_error: null,
            is_muted: false,
            is_monitoring: false,
            input_level: 0,
            output_level: 0,
          };
        }
        return { is_active: false, peers: [], device_problems: statusDeviceProblems };
      }
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

describe('MainScreen の接続の表示', () => {
  it('バックエンドがまだ答えていないとき、サーバーに接続中の表示であること', async () => {
    current = new Promise(() => {});

    render(<MainScreen />);

    expect(await screen.findByTestId('connection-panel-loading')).toBeInTheDocument();
  });

  it('サーバーに繋がっているとき、ルームを作るボタンで session_create を呼ぶこと', async () => {
    render(<MainScreen />);

    fireEvent.click(await screen.findByTestId('connection-panel-create-room'));

    await waitFor(() => expect(callsTo('session_create')).toHaveLength(1));
  });

  // Verifies: REQ-CON-139
  it('招待コードを入れて参加するとき、ミュートの指定を付けずに session_join を呼ぶこと', async () => {
    render(<MainScreen />);

    fireEvent.change(await screen.findByTestId('connection-panel-invite-code'), {
      target: { value: 'ABC234XYZ' },
    });
    fireEvent.click(screen.getByTestId('connection-panel-join'));

    await waitFor(() => expect(callsTo('session_join').map((c) => c.args)).toEqual([{ code: 'ABC234XYZ' }]));
  });

  it('サーバーがテストルームを示しているとき、そのコードへのショートカットを出すこと', async () => {
    current = snapshot({ test_room_invite_code: 'HJK567MNP' });

    render(<MainScreen />);

    expect(await screen.findByTestId('connection-panel-test-room')).toBeInTheDocument();
  });

  it('接続に失敗したとき、理由を出し、やり直しで session_connect を呼ぶこと', async () => {
    current = snapshot({ phase: 'error', connection_id: null, error: 'connection refused' });

    render(<MainScreen />);
    fireEvent.click(await screen.findByTestId('connection-panel-retry'));

    expect(callsTo('session_connect')).toHaveLength(1);
    expect(screen.getByTestId('connection-panel-server-error')).toBeInTheDocument();
  });

  it('接続中の表示で取り消したとき、session_connect を呼ぶこと', async () => {
    current = snapshot({ phase: 'connecting_server', connection_id: null });

    render(<MainScreen />);
    fireEvent.click(await screen.findByTestId('connection-panel-cancel'));

    expect(callsTo('session_connect')).toHaveLength(1);
  });

  it('接続中の表示で取り消したとき、接続先の URL を読み直すこと', async () => {
    current = snapshot({ phase: 'connecting_server', connection_id: null });

    render(<MainScreen />);
    await screen.findByTestId('connection-panel-cancel');
    await waitFor(() => expect(callsTo('config_get_effective_server_url')).toHaveLength(1));
    fireEvent.click(screen.getByTestId('connection-panel-cancel'));

    // Cancelling leaves the phase as it was, so nothing else would read it again.
    await waitFor(() => expect(callsTo('config_get_effective_server_url')).toHaveLength(2));
  });
});

describe('MainScreen のルームの表示', () => {
  it('バックエンドがルームに入ったと伝えたとき、招待コードと参加者を出すこと', async () => {
    render(<MainScreen />);
    await screen.findByTestId('connection-panel-create-room');

    announce(inRoom({ revision: 2 }));

    expect(await screen.findByTestId('room-code')).toHaveTextContent('ABC234XYZ');
    expect(screen.getByTestId('participant-list')).toHaveTextContent('Aki');
    expect(screen.getByTestId('participant-list')).toHaveTextContent('Me');
  });

  // Verifies: REQ-GUI-023
  it('相手の名前が HTML を含むとき、要素にならず文字として表示されること', async () => {
    const payload = '<img src=x onerror=alert(1)>';
    current = inRoom();
    current.room!.participants = [{ id: 'b', name: payload, features: [] }];

    render(<MainScreen />);

    const list = await screen.findByTestId('participant-list');
    expect(list).toHaveTextContent(payload);
    expect(list.querySelector('img')).toBeNull();
  });

  it('古い通知があとから届いたとき、新しい状態のままであること', async () => {
    current = inRoom({ revision: 5 });
    render(<MainScreen />);
    await screen.findByTestId('room-code');

    announce(snapshot({ revision: 4 }));

    expect(screen.getByTestId('room-code')).toBeInTheDocument();
  });

  it('参加者が増えたと伝えられたとき、一覧に加わること', async () => {
    current = inRoom({ revision: 2 });
    render(<MainScreen />);
    await screen.findByTestId('room-code');

    const next = inRoom({ revision: 3 });
    next.room!.participants.push({ id: 'c', name: 'Bo', features: [] });
    announce(next);

    await waitFor(() => expect(screen.getByTestId('participant-list')).toHaveTextContent('Bo'));
  });

  it('退室を確かめたとき、session_leave を呼ぶこと', async () => {
    current = inRoom();
    render(<MainScreen />);

    fireEvent.click(await screen.findByTestId('leave-room'));
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: i18n.t('session.leave.confirmButton') }));

    await waitFor(() => expect(callsTo('session_leave')).toHaveLength(1));
  });

  it('シグナリングが切れて繋ぎ直しているとき、ルームの画面のまま知らせること', async () => {
    current = inRoom({
      connection_id: null,
      signaling_reconnect: 'reconnecting',
    });

    render(<MainScreen />);

    expect(await screen.findByTestId('room-code')).toBeInTheDocument();
    expect(screen.getByText(i18n.t('session.signalingReconnect.inProgress'))).toBeInTheDocument();
  });

  it('繋ぎ直しに失敗したとき、理由を出し、再試行で session_reconnect を呼ぶこと', async () => {
    current = inRoom({
      connection_id: null,
      signaling_reconnect: 'failed',
      signaling_reconnect_error: 'no route to host',
    });

    render(<MainScreen />);
    fireEvent.click(await screen.findByRole('button', { name: i18n.t('session.reconnect.retry') }));

    expect(callsTo('session_reconnect')).toHaveLength(1);
    expect(screen.getByText(/no route to host/)).toBeInTheDocument();
  });
});

describe('MainScreen の受信バッファ調整の知らせ', () => {
  /** Verifies: REQ-LAT-108 */
  it('受信バッファが自動で調整されたとき、設定のバッファサイズと取り違えない言い方で、調整後の ms を知らせること', async () => {
    current = inRoom();
    statusLinks = [link('b', 'connected', { up: 5, down: 5 })];
    statusAudioQuality = { underrun_count: 0, delay_adjustments: 1, adjusted_delay_ms: 5.33 };

    render(<MainScreen />);

    expect(await screen.findByText('Network buffer adjusted to 5.3 ms')).toBeInTheDocument();
  });
});

describe('MainScreen の暗号化の表示', () => {
  /** Verifies: REQ-SEC-006 */
  it('相手のアプリが暗号化できないとき、この相手との音声は暗号化されていないと知らせること', async () => {
    current = inRoom();
    statusEncryption = 'unencrypted';

    render(<MainScreen />);

    expect(await screen.findByText(/Audio with this peer is not encrypted/)).toBeInTheDocument();
  });

  it('知らせが出るとき、フッターの外の上中央の置き場に出て、フッターの接続表示を動かさないこと', async () => {
    current = inRoom();
    statusEncryption = 'unencrypted';

    const { container } = render(<MainScreen />);

    const toast = (await screen.findByText(/Audio with this peer is not encrypted/)).closest('.toast');
    expect(toast?.closest('.toast-stack')).not.toBeNull();
    expect(container.querySelector('.main-footer')?.querySelector('.toast')).toBeNull();
  });

  it('相手との音声が暗号化されているとき、暗号化についての知らせは出さないこと', async () => {
    current = inRoom();
    statusEncryption = 'encrypted';

    render(<MainScreen />);

    await waitFor(() => expect(callsTo('streaming_status').length).toBeGreaterThan(0));
    expect(screen.queryByText(/not encrypted/)).not.toBeInTheDocument();
  });

  it('鍵を合わせている最中のとき、暗号化されていないとは知らせないこと', async () => {
    current = inRoom();
    statusEncryption = 'negotiating';

    render(<MainScreen />);

    await waitFor(() => expect(callsTo('streaming_status').length).toBeGreaterThan(0));
    expect(screen.queryByText(/not encrypted/)).not.toBeInTheDocument();
  });
});

describe('MainScreen の参加者ごとの接続の表示', () => {
  const row = (name: string) =>
    screen.getByText(name, { selector: '.participant__name' }).closest('li')!;

  // Verifies: REQ-LAT-102
  it('3 人のルームで相手ごとに遅延が違うとき、それぞれの行にその人との遅延を出し、いちばん大きい人に印を付けること', async () => {
    current = roomOfThree();
    statusLinks = [
      link('b', 'connected', { up: 4, down: 6 }),
      link('c', 'connected', { up: 21, down: 23 }),
    ];

    render(<MainScreen />);

    await waitFor(() => expect(row('Aki')).toHaveTextContent('↑4ms↓6ms'));
    expect(row('Ben')).toHaveTextContent('↑21ms↓23ms');
    const marked = screen
      .getAllByTestId('participant-latency')
      .filter((latency) => latency.className.includes('participant__latency--highest'));
    expect(marked).toHaveLength(1);
    expect(row('Ben')).toContainElement(marked[0]);
    expect(marked[0]).toHaveAttribute('title', 'Highest latency');
  });

  // Verifies: REQ-LAT-102
  it('相手が 1 人のとき、遅延に印を付けないこと', async () => {
    current = inRoom();
    statusLinks = [link('b', 'connected', { up: 4, down: 6 })];

    render(<MainScreen />);

    await waitFor(() => expect(row('Aki')).toHaveTextContent('↑4ms↓6ms'));
    expect(screen.getByTestId('participant-latency').className).not.toContain('highest');
  });

  /** `link` with this machine's buffers and the peer's, as the backend reports them */
  function withBuffers(
    base: PeerLinkStatus,
    buffers: { capture: number; playback: number; jitter: number; peerCapture: number }
  ): PeerLinkStatus {
    return {
      ...base,
      latency: {
        ...base.latency!,
        upstream: [{ name: 'Capture buffer', ms: buffers.capture, info: null }],
        downstream: [
          { name: 'Jitter buffer', ms: buffers.jitter, info: null },
          { name: 'Playback buffer', ms: buffers.playback, info: null },
        ],
      },
      peer_audio: {
        sample_rate: 48000,
        frame_size: 32,
        codec: 'pcm',
        needs_resampling: false,
        channel_count: 2,
        capture_buffer_ms: buffers.peerCapture,
        playback_buffer_ms: 1,
        jitter_buffer_ms: 2,
      },
    };
  }

  // Verifies: REQ-LAT-102
  it('3 人のルームで一方の行を開いたとき、その人との内訳だけを出し、もう一方の人の値は出さないこと', async () => {
    current = roomOfThree();
    statusLinks = [
      withBuffers(link('b', 'connected', { up: 4, down: 6 }), {
        capture: 0.5, playback: 0.6, jitter: 3, peerCapture: 7,
      }),
      withBuffers(link('c', 'connected', { up: 21, down: 23 }), {
        capture: 0.5, playback: 0.6, jitter: 3, peerCapture: 9,
      }),
    ];

    render(<MainScreen />);
    await waitFor(() => expect(row('Ben')).toHaveTextContent('↑21ms↓23ms'));
    expect(screen.queryByTestId('peer-link-detail')).not.toBeInTheDocument();

    fireEvent.click(within(row('Ben')).getByTestId('participant-toggle'));

    expect(within(row('Ben')).getByTestId('detail-peer-input')).toHaveTextContent('9.0ms');
    expect(within(row('Ben')).getByTestId('detail-network-rtt')).toHaveTextContent('44ms');
    expect(within(row('Aki')).queryByTestId('peer-link-detail')).not.toBeInTheDocument();
    expect(screen.getAllByTestId('peer-link-detail')).toHaveLength(1);
  });

  it('別の人の行を開いたとき、先に開いていた内訳は閉じること', async () => {
    current = roomOfThree();
    statusLinks = [
      link('b', 'connected', { up: 4, down: 6 }),
      link('c', 'connected', { up: 21, down: 23 }),
    ];

    render(<MainScreen />);
    await waitFor(() => expect(row('Aki')).toHaveTextContent('↑4ms↓6ms'));
    fireEvent.click(within(row('Aki')).getByTestId('participant-toggle'));
    fireEvent.click(within(row('Ben')).getByTestId('participant-toggle'));

    expect(within(row('Aki')).queryByTestId('peer-link-detail')).not.toBeInTheDocument();
    expect(within(row('Ben')).getByTestId('peer-link-detail')).toBeInTheDocument();
    expect(within(row('Ben')).getByTestId('participant-toggle')).toHaveAttribute('aria-expanded', 'true');
  });

  it('開いている行をもう一度押したとき、内訳を閉じること', async () => {
    current = inRoom();
    statusLinks = [link('b', 'connected', { up: 4, down: 6 })];

    render(<MainScreen />);
    await waitFor(() => expect(row('Aki')).toHaveTextContent('↑4ms↓6ms'));
    const toggle = within(row('Aki')).getByTestId('participant-toggle');
    fireEvent.click(toggle);
    fireEvent.click(toggle);

    expect(screen.queryByTestId('peer-link-detail')).not.toBeInTheDocument();
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
  });

  it('内訳を開いていた人が退出して戻ってきたとき、閉じた状態で戻ること', async () => {
    current = roomOfThree();
    statusLinks = [
      link('b', 'connected', { up: 4, down: 6 }),
      link('c', 'connected', { up: 21, down: 23 }),
    ];
    const room = current.room!;

    render(<MainScreen />);
    await waitFor(() => expect(row('Ben')).toHaveTextContent('↑21ms↓23ms'));
    fireEvent.click(within(row('Ben')).getByTestId('participant-toggle'));
    announce(inRoom({ revision: 5, room: { ...room, participants: [room.participants[0]] } }));
    await waitFor(() => expect(screen.queryByText('Ben', { selector: '.participant__name' })).toBeNull());
    announce(inRoom({ revision: 6, room }));

    await waitFor(() => expect(row('Ben')).toBeInTheDocument());
    expect(screen.queryByTestId('peer-link-detail')).not.toBeInTheDocument();
  });

  it('自分の行は、押しても開く内訳を持たないこと', async () => {
    current = inRoom();
    statusLinks = [link('b', 'connected', { up: 4, down: 6 })];

    render(<MainScreen />);

    await waitFor(() => expect(row('Aki')).toHaveTextContent('↑4ms↓6ms'));
    expect(screen.getAllByTestId('participant-toggle')).toHaveLength(1);
  });

  // Verifies: REQ-LAT-102
  it('3 人のルームで遅延が違うとき、足もとの接続表示には誰のものか分からない遅延を出さないこと', async () => {
    current = roomOfThree();
    statusLinks = [
      withBuffers(link('b', 'connected', { up: 4, down: 6 }), {
        capture: 0.5, playback: 0.6, jitter: 3, peerCapture: 7,
      }),
      withBuffers(link('c', 'connected', { up: 21, down: 23 }), {
        capture: 0.5, playback: 0.6, jitter: 3, peerCapture: 9,
      }),
    ];

    render(<MainScreen />);
    await waitFor(() => expect(row('Ben')).toHaveTextContent('↑21ms↓23ms'));

    const footer = screen.getByRole('status');
    expect(footer).toHaveTextContent('Connected');
    expect(footer).not.toHaveTextContent(/\d+ms/);
  });

  // Verifies: REQ-CON-134
  it('3 人のルームで 1 人との接続が諦められたとき、その人の行に切断と出し、誰との接続かを添えて再接続を尋ねること', async () => {
    current = roomOfThree();
    statusLinks = [
      link('b', 'connected', { up: 4, down: 6 }),
      link('c', 'failed', { up: 21, down: 23 }),
    ];

    render(<MainScreen />);

    expect(await screen.findByText('Connection to Ben lost. Reconnect?')).toBeInTheDocument();
    expect(row('Ben')).toHaveAttribute('data-link', 'error');
    expect(row('Ben')).toHaveTextContent('Disconnected');
    expect(row('Aki')).toHaveAttribute('data-link', 'connected');
    expect(row('Aki')).toHaveTextContent('↑4ms↓6ms');

    fireEvent.click(screen.getByRole('button', { name: i18n.t('session.reconnect.retry') }));
    await waitFor(() => expect(callsTo('streaming_reconnect')).toHaveLength(1));
  });

  // Verifies: REQ-CON-110
  it('相手が 1 人で接続が諦められたとき、名前を添えずに再接続を尋ねること', async () => {
    current = inRoom();
    statusLinks = [link('b', 'failed', { up: 4, down: 6 })];

    render(<MainScreen />);

    expect(await screen.findByText('Connection lost. Reconnect?')).toBeInTheDocument();
  });

  // Verifies: REQ-CON-133
  it('入ったばかりでまだ音声を繋いでいない参加者の行には、接続中と出すこと', async () => {
    current = roomOfThree();
    statusLinks = [link('b', 'connected', { up: 4, down: 6 }), link('c', 'checkingconnectivity')];

    render(<MainScreen />);

    await waitFor(() => expect(row('Aki')).toHaveAttribute('data-link', 'connected'));
    expect(row('Ben')).toHaveAttribute('data-link', 'connecting');
    expect(row('Ben')).toHaveTextContent('Connecting...');
  });

  it('1 人からの音が途切れているとき、その人の行に不安定と出し、ほかの人の行は遅延のままであること', async () => {
    current = roomOfThree();
    statusLinks = [
      link('b', 'reconnecting', { up: 4, down: 6 }),
      link('c', 'connected', { up: 21, down: 23 }),
    ];

    render(<MainScreen />);

    await waitFor(() => expect(row('Aki')).toHaveAttribute('data-link', 'unstable'));
    expect(row('Aki')).toHaveTextContent('Unstable');
    expect(row('Ben')).toHaveTextContent('↑21ms↓23ms');
  });

  // Verifies: REQ-GUI-009
  it('3 人のルームのとき、参加者ごとのメーターがその人の音のレベルを示すこと', async () => {
    current = roomOfThree();
    mixerNow = faders({
      peers: {
        b: { volume: 80, pan: 0, muted: false },
        c: { volume: 80, pan: 0, muted: false },
      },
    });
    statusLinks = [
      link('b', 'connected', { up: 4, down: 6, level: 30 }),
      link('c', 'connected', { up: 4, down: 6, level: 70 }),
    ];

    render(<MainScreen />);

    const peak = (id: string) =>
      document
        .querySelector(`[data-testid="channel-strip"][data-channel-id="${id}"] [data-testid="channel-peak"]`)!
        .getAttribute('data-peak');
    await waitFor(() => expect(peak('c')).toBe('70'));
    expect(peak('b')).toBe('30');
  });

  /** Verifies: REQ-SEC-006 */
  it('3 人のルームで 1 人のアプリが暗号化できないとき、誰との音声が暗号化されていないかを知らせること', async () => {
    current = roomOfThree();
    statusLinks = [
      link('b', 'connected', { up: 4, down: 6 }),
      link('c', 'connected', { up: 4, down: 6, encryption: 'unencrypted' }),
    ];

    render(<MainScreen />);

    expect(await screen.findByText(/Audio with Ben is not encrypted/)).toBeInTheDocument();
  });
});

describe('MainScreen の新しい版の知らせ', () => {
  /** Verifies: REQ-UPD-021 */
  it('自分で入れ替えられない配布形態が新しい版を見つけているとき、その版を入れ直すよう知らせること', async () => {
    newerRelease = '1.2.3';

    render(<MainScreen />);

    expect(await screen.findByText(/A new version, 1\.2\.3, is out/)).toBeInTheDocument();
  });

  /** Verifies: REQ-UPD-021 */
  it('起動のあとで新しい版が見つかったとき、知らせが出ること', async () => {
    render(<MainScreen />);
    await waitFor(() => expect(callsTo('update_notice_get')).toHaveLength(1));

    act(() => listeners.get('update:available')!({ payload: '1.2.3' }));

    expect(await screen.findByText(/A new version, 1\.2\.3, is out/)).toBeInTheDocument();
  });

  /** Verifies: REQ-UPD-021 */
  it('新しい版が見つかっていないとき、知らせは出さないこと', async () => {
    render(<MainScreen />);

    await waitFor(() => expect(callsTo('update_notice_get')).toHaveLength(1));
    expect(screen.queryByText(/is out/)).not.toBeInTheDocument();
  });

  /** Verifies: REQ-UPD-021 */
  it('手伝っている画面のとき、新しい版を尋ねないこと', async () => {
    newerRelease = '1.2.3';

    render(<MainScreen helper={{ name: 'Aki' }} />);

    await waitFor(() => expect(callsTo('session_get').length).toBeGreaterThan(0));
    expect(callsTo('update_notice_get')).toHaveLength(0);
    expect(screen.queryByText(/is out/)).not.toBeInTheDocument();
  });
});

describe('MainScreen のデバイスの問題の表示', () => {
  /** Verifies: REQ-AUD-123 */
  it('入力デバイスが応答しないとき、セッションが始まっていなくても、その名前を添えて知らせること', async () => {
    current = inRoom();
    statusDeviceProblems = [{ side: 'input', trouble: 'unresponsive', device: 'AG06/AG03', sample_rate: 48000 }];

    render(<MainScreen />);

    const message = await screen.findByText(/Input device isn't responding/);
    expect(message).toHaveTextContent('(AG06/AG03)');
  });

  /** Verifies: REQ-AUD-123 */
  it('出力デバイスを開けなかったとき、名前が分からなくても、出力の問題として知らせること', async () => {
    current = inRoom();
    statusDeviceProblems = [{ side: 'output', trouble: 'failed', device: null, sample_rate: 48000 }];

    render(<MainScreen />);

    expect(await screen.findByText(/Couldn't open the output device/)).toBeInTheDocument();
  });

  /** Verifies: REQ-AUD-125 */
  it('入力デバイスが 48000Hz で開けないとき、デバイスの名前とレートを添えて、形式を合わせるよう知らせること', async () => {
    current = inRoom();
    statusDeviceProblems = [
      { side: 'input', trouble: 'unsupported_format', device: 'USB AUDIO CODEC', sample_rate: 48000 },
    ];

    render(<MainScreen />);

    const message = await screen.findByText(/Couldn't open the input device/);
    expect(message).toHaveTextContent(
      "Couldn't open the input device (USB AUDIO CODEC) at 48000 Hz. In your sound settings, set this device's format to 48000 Hz"
    );
  });

  /** Verifies: REQ-AUD-128 */
  it('入力の音が止まって開き直しても戻らないとき、デバイスの名前を添えて、入り直すよう知らせること', async () => {
    current = inRoom();
    statusDeviceProblems = [{ side: 'input', trouble: 'stalled', device: 'null sink', sample_rate: 48000 }];

    render(<MainScreen />);

    const message = await screen.findByText(/stopped delivering audio/);
    expect(message).toHaveTextContent(
      "The input device (null sink) stopped delivering audio, and opening it again didn't help. Leave the room and join again"
    );
  });

  /** Verifies: REQ-AUD-128 */
  it('出力の音が止まって開き直しても戻らないとき、出力の問題として知らせること', async () => {
    current = inRoom();
    statusDeviceProblems = [{ side: 'output', trouble: 'stalled', device: null, sample_rate: 48000 }];

    render(<MainScreen />);

    expect(await screen.findByText(/The output device stopped playing audio/)).toBeInTheDocument();
  });

  /** Verifies: REQ-AUD-123 */
  it('デバイスの問題が解けたとき、知らせが消えること', async () => {
    current = inRoom();
    statusDeviceProblems = [{ side: 'input', trouble: 'unresponsive', device: null, sample_rate: 48000 }];
    render(<MainScreen />);
    await screen.findByText(/Input device isn't responding/);

    statusDeviceProblems = [];

    await waitFor(() =>
      expect(screen.queryByText(/Input device isn't responding/)).not.toBeInTheDocument()
    );
  });
});

describe('MainScreen のミキサー', () => {
  const peerFader = () => screen.findByLabelText(i18n.t('mixer.channel.volumeLabel', { name: 'Aki' }));

  // Verifies: REQ-RMT-031
  it('バックエンドのフェーダーの位置が相手の音量 30 のとき、その相手のフェーダーが 30 を示すこと', async () => {
    current = inRoom();
    mixerNow = faders({ peers: { b: { volume: 30, pan: 0, muted: false } } });

    render(<MainScreen />);

    expect(await peerFader()).toHaveValue('30');
  });

  // Verifies: REQ-RMT-031
  it('バックエンドが相手の音量が変わったと伝えたとき、フェーダーがその位置に動くこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    await waitFor(() => expect(listeners.has('mixer:changed')).toBe(true));
    expect(await peerFader()).toHaveValue('80');

    act(() =>
      listeners.get('mixer:changed')!({
        payload: faders({ revision: 2, peers: { b: { volume: 45, pan: 0, muted: false } } }),
      })
    );

    await waitFor(async () => expect(await peerFader()).toHaveValue('45'));
  });

  // Verifies: REQ-RMT-031
  it('古いフェーダーの通知があとから届いたとき、新しい位置のままであること', async () => {
    current = inRoom();
    mixerNow = faders({ revision: 5, peers: { b: { volume: 45, pan: 0, muted: false } } });
    render(<MainScreen />);
    expect(await peerFader()).toHaveValue('45');

    act(() =>
      listeners.get('mixer:changed')!({
        payload: faders({ revision: 4, peers: { b: { volume: 10, pan: 0, muted: false } } }),
      })
    );

    expect(await peerFader()).toHaveValue('45');
  });

  // Verifies: REQ-RMT-031
  it('相手のフェーダーを動かしたとき、その参加者の ID と値で mixer_set_peer_volume を呼ぶこと', async () => {
    current = inRoom();
    render(<MainScreen />);

    fireEvent.change(await peerFader(), { target: { value: '30' } });

    await waitFor(() =>
      expect(callsTo('mixer_set_peer_volume')).toEqual([
        { cmd: 'mixer_set_peer_volume', args: { peerId: 'b', volume: 30 } },
      ])
    );
  });

  // Verifies: REQ-RMT-031
  it('自分のフェーダーを動かしたとき、mixer_set_local_volume を呼ぶこと', async () => {
    current = inRoom();
    render(<MainScreen />);

    fireEvent.change(await screen.findByLabelText(i18n.t('mixer.channel.volumeLabel', { name: i18n.t('mixer.self') })), {
      target: { value: '55' },
    });

    await waitFor(() =>
      expect(callsTo('mixer_set_local_volume')).toEqual([
        { cmd: 'mixer_set_local_volume', args: { volume: 55 } },
      ])
    );
  });

  // Verifies: REQ-RMT-031
  it('相手のミュートを押したとき、mixer_set_peer_muted をミュートにして呼び、戻すと外すこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    const strip = (await screen.findAllByTestId('channel-strip')).find(
      (el) => el.getAttribute('data-channel-id') === 'b'
    )!;

    fireEvent.click(within(strip).getByTestId('channel-mute'));

    await waitFor(() =>
      expect(callsTo('mixer_set_peer_muted')).toEqual([
        { cmd: 'mixer_set_peer_muted', args: { peerId: 'b', muted: true } },
      ])
    );
  });
});

describe('MainScreen の招待リンク', () => {
  async function withConnection() {
    render(<MainScreen />);
    await screen.findByTestId('connection-panel-create-room');
    await waitFor(() => expect(openUrl.handler).not.toBeNull());
  }

  // Verifies: REQ-CON-103
  // Verifies: REQ-CON-139
  it('招待リンクが届いたとき、そのコードをミュートで入る指定を付けて session_join を呼ぶこと', async () => {
    await withConnection();

    act(() => openUrl.handler!(['jamuru://join/abc234xyz']));

    await waitFor(() =>
      expect(callsTo('session_join').map((c) => c.args)).toEqual([
        { code: 'ABC234XYZ', muted: true },
      ])
    );
  });

  it('コードが壊れた招待リンクが届いたとき、エラーを出して参加しないこと', async () => {
    await withConnection();

    act(() => openUrl.handler!(['jamuru://join/nope']));

    expect(await screen.findByTestId('connection-panel-error')).toBeInTheDocument();
    expect(callsTo('session_join')).toHaveLength(0);
  });

  it('サーバーに繋がっていないとき、リンクの受け取りを登録しないこと', async () => {
    current = snapshot({ phase: 'connecting_server', connection_id: null });

    render(<MainScreen />);
    await screen.findByTestId('connection-panel-loading');
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(openUrl.handler).toBeNull();
  });
});

describe('MainScreen の入室前の名前の確認', () => {
  async function pressCreate() {
    fireEvent.click(await screen.findByTestId('connection-panel-create-room'));
    return screen.findByRole('dialog', { name: 'Enter your name' });
  }

  // Verifies: REQ-CON-141
  it('名前を確認する設定のとき、ルームを作る前に名前を聞き、答えるまで session_create を呼ばないこと', async () => {
    savedConfig = { peer_name: 'Aki', ask_name_on_entry: true };
    render(<MainScreen />);

    await pressCreate();

    expect(screen.getByLabelText('Display name')).toHaveValue('Aki');
    expect(callsTo('session_create')).toHaveLength(0);
  });

  // Verifies: REQ-CON-141
  it('名前をまだ決めていないとき、初期値は「ゲスト」であること', async () => {
    savedConfig = { peer_name: 'User', ask_name_on_entry: true };
    render(<MainScreen />);

    await pressCreate();

    expect(screen.getByLabelText('Display name')).toHaveValue('Guest');
  });

  // Verifies: REQ-CON-141
  it('名前を入れて決めたとき、その名前を保存してから session_create を呼ぶこと', async () => {
    savedConfig = { peer_name: 'Aki', ask_name_on_entry: true };
    render(<MainScreen />);
    await pressCreate();

    fireEvent.change(screen.getByLabelText('Display name'), { target: { value: 'Taro' } });
    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Create Room' }));

    await waitFor(() => expect(callsTo('session_create')).toHaveLength(1));
    expect(calls.map((c) => c.cmd).filter((cmd) => cmd.startsWith('config_set') || cmd === 'session_create')).toEqual([
      'config_set_peer_name',
      'session_create',
    ]);
    expect(callsTo('config_set_peer_name').map((c) => c.args)).toEqual([{ name: 'Taro' }]);
    expect(callsTo('config_set_ask_name_on_entry')).toHaveLength(0);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  // Verifies: REQ-CON-142
  it('「次からこの名前を使用する」を選んで決めたとき、確認を止める設定も保存すること', async () => {
    savedConfig = { peer_name: 'Aki', ask_name_on_entry: true };
    render(<MainScreen />);
    await pressCreate();

    fireEvent.click(screen.getByLabelText('Use this name from now on'));
    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Create Room' }));

    await waitFor(() => expect(callsTo('session_create')).toHaveLength(1));
    expect(callsTo('config_set_ask_name_on_entry').map((c) => c.args)).toEqual([{ enabled: false }]);
  });

  // Verifies: REQ-CON-142
  it('名前を確認しない設定のとき、聞かずにそのまま session_create を呼ぶこと', async () => {
    savedConfig = { peer_name: 'Aki', ask_name_on_entry: false };
    render(<MainScreen />);

    fireEvent.click(await screen.findByTestId('connection-panel-create-room'));

    await waitFor(() => expect(callsTo('session_create')).toHaveLength(1));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  // Verifies: REQ-CON-141
  it('キャンセルしたとき、ルームを作らず名前も変えないこと', async () => {
    savedConfig = { peer_name: 'Aki', ask_name_on_entry: true };
    render(<MainScreen />);
    await pressCreate();

    fireEvent.change(screen.getByLabelText('Display name'), { target: { value: 'Taro' } });
    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }));

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(callsTo('session_create')).toHaveLength(0);
    expect(callsTo('config_set_peer_name')).toHaveLength(0);
  });

  // Verifies: REQ-CON-141
  it('招待コードで参加するとき、名前を聞いてから、ミュートの指定を付けずに session_join を呼ぶこと', async () => {
    savedConfig = { peer_name: 'Aki', ask_name_on_entry: true };
    render(<MainScreen />);
    fireEvent.change(await screen.findByTestId('connection-panel-invite-code'), {
      target: { value: 'ABC234XYZ' },
    });
    fireEvent.click(screen.getByTestId('connection-panel-join'));
    await screen.findByRole('dialog', { name: 'Enter your name' });
    expect(callsTo('session_join')).toHaveLength(0);

    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Join' }));

    await waitFor(() => expect(callsTo('session_join').map((c) => c.args)).toEqual([{ code: 'ABC234XYZ' }]));
  });

  // Verifies: REQ-CON-141
  // Verifies: REQ-CON-139
  it('招待リンクが届いたとき、名前を聞いてから、ミュートの指定を付けて session_join を呼ぶこと', async () => {
    savedConfig = { peer_name: 'Aki', ask_name_on_entry: true };
    render(<MainScreen />);
    await screen.findByTestId('connection-panel-create-room');
    await waitFor(() => expect(openUrl.handler).not.toBeNull());

    act(() => openUrl.handler!(['jamuru://join/abc234xyz']));
    await screen.findByRole('dialog', { name: 'Enter your name' });
    expect(callsTo('session_join')).toHaveLength(0);
    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Join' }));

    await waitFor(() =>
      expect(callsTo('session_join').map((c) => c.args)).toEqual([{ code: 'ABC234XYZ', muted: true }])
    );
  });

  // Verifies: REQ-CON-141
  it('名前を保存できなかったとき、ルームを作らず、理由を出して聞き直せること', async () => {
    savedConfig = { peer_name: 'Aki', ask_name_on_entry: true };
    const answer = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'config_set_peer_name') throw new Error('disk full');
      return answer(cmd, args);
    });
    render(<MainScreen />);
    await pressCreate();

    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Create Room' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('Could not save the name');
    expect(callsTo('session_create')).toHaveLength(0);
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });
});

describe('MainScreen の手伝われているあいだの動きの表示', () => {
  const presenceIn = (payload: unknown) => act(() => listeners.get('help:presence-in')!({ payload }));
  const helpEvent = (payload: unknown) => act(() => listeners.get('session:settings-help')!({ payload }));

  async function helped() {
    current = inRoom();
    render(<MainScreen />);
    await screen.findByTestId('room-code');
    helpEvent({ type: 'started', role: 'helped', peer: 'b', peer_name: 'Aki' });
    await screen.findByTestId('settings-help-bar');
    await waitFor(() => expect(listeners.has('help:presence-in')).toBe(true));
  }

  /** Gives a part the room it would take (jsdom has no layout) */
  function place(el: Element, left: number, top: number, width: number, height: number) {
    el.getBoundingClientRect = () =>
      ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) }) as DOMRect;
  }

  // Verifies: REQ-RMT-034
  it('手伝われていないとき、画面の動きは送られず、聞きにも行かないこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    const leave = await screen.findByTestId('leave-room');
    calls = [];

    fireEvent.pointerMove(leave, { clientX: 1, clientY: 1 });
    await new Promise((resolve) => setTimeout(resolve, 150));

    expect(callsTo('help_presence_publish')).toHaveLength(0);
    expect(listeners.has('help:presence-in')).toBe(false);
  });

  // Verifies: REQ-RMT-034
  it('手伝われているあいだ、ボタンの上にポインタを置くと、部品の名前と位置が手伝う人の窓へ向けて出されること', async () => {
    await helped();
    calls = [];
    const leave = screen.getByTestId('leave-room');
    place(leave, 0, 0, 100, 40);

    fireEvent.pointerMove(leave, { clientX: 50, clientY: 20 });

    await waitFor(() =>
      expect(callsTo('help_presence_publish').slice(-1)[0]?.args).toEqual({
        presence: { hover: { id: 'leave-room', x: 0.5, y: 0.5 }, focus: null },
      })
    );
  });

  // Verifies: REQ-RMT-034
  it('手伝う人が部品を指しているとき、その部品に名前つきの枠が出て、手伝いが終わると消えること', async () => {
    await helped();
    const leave = screen.getByTestId('leave-room');
    place(leave, 12, 34, 100, 40);

    presenceIn({ hover: { id: 'leave-room', x: 0.25, y: 0.5 }, focus: null });

    const mark = await screen.findByTestId('presence-mark-hover');
    expect(mark).toHaveStyle({ left: '12px', top: '34px', width: '100px', height: '40px' });
    expect(mark).toHaveTextContent('Aki');

    helpEvent({ type: 'ended', role: 'helped', peer: 'b', reason: 'peer_stopped' });
    await waitFor(() => expect(screen.queryByTestId('presence-marks')).not.toBeInTheDocument());
    expect(listeners.has('help:presence-in')).toBe(false);
  });

  // Verifies: REQ-RMT-034
  it('手伝う人が設定パネルを開いたとき、同じパネルが手伝われる人の画面にも出て、閉じると消え、手伝われる人が閉じても次に開かれるまで出ないこと', async () => {
    await helped();
    const panelOf = () => screen.getByRole('dialog', { hidden: true });
    expect(panelOf()).not.toHaveClass('side-panel--open');

    presenceIn({ hover: null, focus: null, panel: true });
    await waitFor(() => expect(panelOf()).toHaveClass('side-panel--open'));
    expect(panelOf()).toHaveAttribute('aria-label', 'Audio settings Aki has open');
    expect(panelOf()).toHaveAttribute('aria-modal', 'false');
    expect(callsTo('settings_get').length).toBeGreaterThan(0);

    fireEvent.click(within(panelOf()).getByRole('button', { name: 'Close', hidden: true }));
    await waitFor(() => expect(panelOf()).not.toHaveClass('side-panel--open'));
    presenceIn({ hover: null, focus: null, panel: true });
    expect(panelOf()).not.toHaveClass('side-panel--open');

    presenceIn({ hover: null, focus: null, panel: false });
    presenceIn({ hover: null, focus: null, panel: true });
    await waitFor(() => expect(panelOf()).toHaveClass('side-panel--open'));

    presenceIn({ hover: null, focus: null, panel: false });
    await waitFor(() => expect(panelOf()).not.toHaveClass('side-panel--open'));
  });
});

describe('MainScreen のメニューバーからの操作', () => {
  /** The menu bar asks the screen to do `action`. */
  function chooseFromMenu(action: string) {
    act(() => listeners.get('menu:action')!({ payload: action }));
  }

  // Verifies: REQ-MNU-005
  it('メニューで「ルームを作成」を選んだとき、session_create を呼ぶこと', async () => {
    render(<MainScreen />);
    await screen.findByTestId('connection-panel-create-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('create_room');

    await waitFor(() => expect(callsTo('session_create')).toHaveLength(1));
  });

  // Verifies: REQ-MNU-005
  it('メニューで「招待コードで参加」を選んだとき、招待コードの入力欄にカーソルが移ること', async () => {
    render(<MainScreen />);
    const input = await screen.findByTestId('connection-panel-invite-code');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('join_room');

    expect(input).toHaveFocus();
  });

  // Verifies: REQ-MNU-005
  it('メニューで「テストルームに入る」を選んだとき、サーバーが示したコードで session_join を呼ぶこと', async () => {
    current = snapshot({ test_room_invite_code: 'HJK567MNP' });
    render(<MainScreen />);
    await screen.findByTestId('connection-panel-test-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('test_room');

    await waitFor(() => expect(callsTo('session_join').map((c) => c.args)).toEqual([{ code: 'HJK567MNP' }]));
  });

  // Verifies: REQ-MNU-005
  it('ルームの中でメニューの「退室」を選んだとき、退室の確認を出し、まだ退室しないこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    await screen.findByTestId('leave-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('leave_room');

    expect(await screen.findByRole('dialog')).toBeInTheDocument();
    expect(callsTo('session_leave')).toHaveLength(0);
  });

  // Verifies: REQ-CON-137
  it('ルームの中でウィンドウを閉じようとしたとき、退室の確認を出し、まだ退室も終了もしないこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    await screen.findByTestId('leave-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('quit_requested');

    expect(await screen.findByRole('dialog')).toBeInTheDocument();
    expect(callsTo('session_leave')).toHaveLength(0);
    expect(callsTo('window_quit_app')).toHaveLength(0);
  });

  // Verifies: REQ-CON-137
  it('ウィンドウを閉じようとして出た確認で退室を選んだとき、session_leave のあとで window_quit_app を呼ぶこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    await screen.findByTestId('leave-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('quit_requested');
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: i18n.t('session.leave.quitButton') }));

    await waitFor(() => expect(callsTo('window_quit_app')).toHaveLength(1));
    expect(calls.map((c) => c.cmd).filter((c) => c === 'session_leave' || c === 'window_quit_app')).toEqual([
      'session_leave',
      'window_quit_app',
    ]);
  });

  // Verifies: REQ-CON-137
  it('ウィンドウを閉じようとして出た確認をキャンセルしたとき、退室も終了もしないこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    await screen.findByTestId('leave-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('quit_requested');
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: i18n.t('common.button.cancel') }));

    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(callsTo('session_leave')).toHaveLength(0);
    expect(callsTo('window_quit_app')).toHaveLength(0);
  });

  // Verifies: REQ-CON-137
  it('キャンセルしたあとに退室ボタンから退室したとき、アプリを終了しないこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    await screen.findByTestId('leave-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('quit_requested');
    fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: i18n.t('common.button.cancel') })
    );
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    fireEvent.click(screen.getByTestId('leave-room'));
    fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: i18n.t('session.leave.confirmButton') })
    );

    await waitFor(() => expect(callsTo('session_leave')).toHaveLength(1));
    expect(callsTo('window_quit_app')).toHaveLength(0);
  });

  // Verifies: REQ-CON-137
  it('終了のための退室に失敗したとき、それでもアプリを終了すること', async () => {
    current = inRoom();
    const base = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'session_leave') {
        calls.push({ cmd, args });
        throw new Error('leave failed');
      }
      return base(cmd, args);
    });
    render(<MainScreen />);
    await screen.findByTestId('leave-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('quit_requested');
    fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: i18n.t('session.leave.quitButton') })
    );

    await waitFor(() => expect(callsTo('session_leave')).toHaveLength(1));
    await waitFor(() => expect(callsTo('window_quit_app')).toHaveLength(1));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  /** The `hold` arguments `streaming_hold_mute` was called with, in order. */
  const holds = () => callsTo('streaming_hold_mute').map((c) => c.args?.hold);

  // Verifies: REQ-CON-138
  it('退室の確認を出している間、マイクのミュートを保つこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    await screen.findByTestId('leave-room');

    fireEvent.click(screen.getByTestId('leave-room'));
    await screen.findByRole('dialog');

    await waitFor(() => expect(holds()).toEqual([true]));
  });

  // Verifies: REQ-CON-138
  it('退室の確認をキャンセルしたとき、ミュートの保持をやめて元に戻すこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    fireEvent.click(await screen.findByTestId('leave-room'));
    const dialog = await screen.findByRole('dialog');

    fireEvent.click(within(dialog).getByRole('button', { name: i18n.t('common.button.cancel') }));

    await waitFor(() => expect(holds()).toEqual([true, false]));
  });

  // Verifies: REQ-CON-138
  it('退室したとき、ミュートの保持をやめて元に戻すこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    fireEvent.click(await screen.findByTestId('leave-room'));
    const dialog = await screen.findByRole('dialog');

    fireEvent.click(within(dialog).getByRole('button', { name: i18n.t('session.leave.confirmButton') }));

    await waitFor(() => expect(holds()).toEqual([true, false]));
  });

  // Verifies: REQ-CON-138
  it('ウィンドウを閉じようとして出た確認から終了するとき、終了までミュートの保持をやめないこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    await screen.findByTestId('leave-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('quit_requested');
    fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: i18n.t('session.leave.quitButton') })
    );

    await waitFor(() => expect(callsTo('window_quit_app')).toHaveLength(1));
    expect(holds()).toEqual([true]);
  });

  // Verifies: REQ-CON-137
  it('ルームの外で終了の確認が届いたとき(すでに退室していたとき)、確認なしで window_quit_app を呼ぶこと', async () => {
    render(<MainScreen />);
    await screen.findByTestId('connection-panel-create-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('quit_requested');

    expect(callsTo('window_quit_app')).toHaveLength(1);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  // Verifies: REQ-MNU-005
  it('ルームの中でメニューの「招待コードをコピー」を選んだとき、そのルームの招待コードをクリップボードに入れること', async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    current = inRoom();
    render(<MainScreen />);
    await screen.findByTestId('leave-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('copy_invite_code');

    expect(writeText).toHaveBeenCalledWith('ABC234XYZ');
  });

  // Verifies: REQ-MNU-005
  it('ルームの中でメニューの「設定依頼をする」を選んだとき、依頼の画面を開くこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    await screen.findByTestId('leave-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('ask_for_help');

    expect(await screen.findByTestId('settings-help-ask')).toBeInTheDocument();
  });

  // Verifies: REQ-MNU-005
  it('ルームの中にいないのにメニューから退室が届いたとき、何もしないこと', async () => {
    render(<MainScreen />);
    await screen.findByTestId('connection-panel-create-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('leave_room');

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  // Verifies: REQ-MNU-005
  it('ルームの中にいるのにメニューからルームの作成が届いたとき、作らないこと', async () => {
    current = inRoom();
    render(<MainScreen />);
    await screen.findByTestId('leave-room');
    await waitFor(() => expect(listeners.has('menu:action')).toBe(true));

    chooseFromMenu('create_room');

    expect(callsTo('session_create')).toHaveLength(0);
  });
});
