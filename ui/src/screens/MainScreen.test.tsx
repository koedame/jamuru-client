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
import type { DeviceProblem, MixerSnapshot, SessionSnapshot } from '../lib/tauri';

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
    streaming_peer_id: null,
    ...changes,
  };
}

function inRoom(changes: Partial<SessionSnapshot> = {}): SessionSnapshot {
  return snapshot({
    phase: 'connected',
    room: {
      room_id: 'room-1',
      invite_code: 'ABC234',
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
      case 'streaming_status':
        return { is_active: false, device_problems: statusDeviceProblems };
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

describe('MainScreen の接続の表示', () => {
  it('バックエンドがまだ答えていないとき、サーバーに接続中の表示であること', async () => {
    current = new Promise(() => {});

    render(<MainScreen />);

    expect(await screen.findByTestId('connection-panel-loading')).toBeInTheDocument();
  });

  it('サーバーに繋がっているとき、ルームを作るボタンで session_create を呼ぶこと', async () => {
    render(<MainScreen />);

    fireEvent.click(await screen.findByTestId('connection-panel-create-room'));

    expect(callsTo('session_create')).toHaveLength(1);
  });

  it('招待コードを入れて参加するとき、そのコードで session_join を呼ぶこと', async () => {
    render(<MainScreen />);

    fireEvent.change(await screen.findByTestId('connection-panel-invite-code'), {
      target: { value: 'ABC234' },
    });
    fireEvent.click(screen.getByTestId('connection-panel-join'));

    expect(callsTo('session_join').map((c) => c.args)).toEqual([{ code: 'ABC234' }]);
  });

  it('サーバーがテストルームを示しているとき、そのコードへのショートカットを出すこと', async () => {
    current = snapshot({ test_room_invite_code: 'TEST22' });

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

    expect(await screen.findByTestId('room-code')).toHaveTextContent('ABC234');
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

  it('招待リンクが届いたとき、そのコードで session_join を呼ぶこと', async () => {
    await withConnection();

    act(() => openUrl.handler!(['jamjam://join/abc234']));

    expect(callsTo('session_join').map((c) => c.args)).toEqual([{ code: 'ABC234' }]);
  });

  it('コードが壊れた招待リンクが届いたとき、エラーを出して参加しないこと', async () => {
    await withConnection();

    act(() => openUrl.handler!(['jamjam://join/nope']));

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
