/**
 * HelperScreen: the window someone helping works in (ADR-044 §5).
 *
 * The window is the helped app's own screen, drawn from the helped app's state
 * over the relay, so what these tests look at is what it asks of the helped app
 * (every command goes out as a call to it) and what it does not offer.
 * What the helped app allows is `src-tauri/src/rpc` and is tested there.
 */

import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

import i18n from '../i18n';
import en from '../../locales/en.json';
import { setBackend, tauriBackend } from '../lib/backend';
import { helperBackend, REMOTE_EVENT } from '../lib/helperBackend';
import type { SessionSnapshot } from '../lib/tauri';
import { audioSettings } from '../components/SettingsPanel/audioSettingsFixture';
import { HelperScreen } from './HelperScreen';

const invoke = vi.hoisted(() => vi.fn());
/** Who listens for each event name (every subscription to the helped app's events is one). */
const listeners = vi.hoisted(() => new Map<string, Array<(event: { payload: unknown }) => void>>());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, [...(listeners.get(name) ?? []), handler]);
    return Promise.resolve(() =>
      listeners.set(name, (listeners.get(name) ?? []).filter((held) => held !== handler))
    );
  }),
  emit: vi.fn(),
  emitTo: vi.fn(),
}));
vi.mock('@tauri-apps/plugin-deep-link', () => ({
  getCurrent: vi.fn(() => Promise.resolve(null)),
  onOpenUrl: vi.fn(() => Promise.resolve(() => {})),
}));

function inRoom(): SessionSnapshot {
  return {
    revision: 1,
    phase: 'connected',
    connection_id: 1,
    test_room_invite_code: null,
    joining_code: null,
    error: null,
    room: {
      room_id: 'room-1',
      invite_code: 'ABC234',
      peer_id: 'bo-id',
      peer_name: 'Bo',
      participants: [{ id: 'aki-id', name: 'Aki', features: ['peer_message'] }],
    },
    signaling_reconnect: 'idle',
    signaling_reconnect_error: null,
    streaming_peer_id: null,
  };
}

/** The calls the window made of the helped app, by method, with their parameters. */
let asked: Array<{ method: string; params: Record<string, unknown> }>;
/** Commands the window kept for itself. */
let own: string[];
let helpedSettings = audioSettings();
/** What the helped app answers `settings_change` with, or fails with. */
let onChange: (params: Record<string, unknown>) => unknown;

function askedOf(method: string) {
  return asked.filter((c) => c.method === method);
}

/** The helped app announces an event. */
function hear(name: string, payload: unknown) {
  act(() => (listeners.get(REMOTE_EVENT) ?? []).forEach((heard) => heard({ payload: { name, payload } })));
}

beforeAll(async () => {
  await i18n.changeLanguage('en');
});

beforeEach(() => {
  asked = [];
  own = [];
  helpedSettings = audioSettings({ revision: 1, buffer_size: 64 });
  onChange = () => audioSettings({ revision: 2, buffer_size: 128 });
  listeners.clear();
  setBackend(helperBackend);
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd !== 'help_call') {
      own.push(cmd);
      if (cmd === 'help_window_info') return { peer_name: 'Bo' };
      return undefined;
    }
    const { method, params } = args as { method: string; params: Record<string, unknown> };
    asked.push({ method, params });
    switch (method) {
      case 'session_get':
        return inRoom();
      case 'settings_get':
        return helpedSettings;
      case 'settings_change': {
        const result = onChange(params);
        if (result instanceof Error) throw { code: 'failed', message: result.message };
        return result;
      }
      case 'streaming_status':
        return { is_active: false };
      case 'config_get_sample_rate':
        return 48000;
      case 'config_get_transmit_channels':
        return 2;
      default:
        return undefined;
    }
  });
});

afterEach(() => {
  setBackend(tauriBackend);
});

describe('HelperScreen', () => {
  // Verifies: REQ-RMT-002
  it('手伝いの画面を開いたとき、相手の名前を示す帯と、相手のルームの画面が出ること', async () => {
    render(<HelperScreen />);

    expect(await screen.findByTestId('settings-help-window-banner')).toHaveTextContent(
      "You are working in Bo's app."
    );
    expect(await screen.findByTestId('room-code')).toHaveTextContent('ABC234');
    expect(await screen.findByTestId('participant-list')).toHaveTextContent('Aki');
  });

  // Verifies: REQ-RMT-002
  it('画面が相手のアプリの状態を読むとき、呼びはすべて手伝いの口へ向かい、自分のアプリの状態を読まないこと', async () => {
    render(<HelperScreen />);
    await screen.findByTestId('room-code');

    expect(askedOf('session_get')).toHaveLength(1);
    expect(askedOf('streaming_status').length).toBeGreaterThan(0);
    // The window's own business is answered here; nothing else is.
    expect(own.filter((cmd) => cmd !== 'help_call' && cmd !== 'help_window_info')).toEqual([]);
  });

  // Verifies: REQ-RMT-025
  it('画面を出すとき、相手の代わりに話す・抜ける操作は出さず、相手の履歴・接続先は聞かないこと', async () => {
    render(<HelperScreen />);
    await screen.findByTestId('room-code');

    expect(screen.queryByTestId('leave-room')).not.toBeInTheDocument();
    expect(screen.queryByTestId('settings-help-offer')).not.toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: /message/i })).not.toBeInTheDocument();
    const methods = asked.map((c) => c.method);
    for (const forbidden of [
      'config_get_connection_history',
      'config_get_effective_server_url',
      'signaling_send_chat',
      'session_leave',
    ]) {
      expect(methods).not.toContain(forbidden);
    }
    // Its size is its own to set, not the helped app's.
    expect(own).not.toContain('window_resize_main');
  });

  // Verifies: REQ-RMT-002
  it('設定ボタンを押したとき、相手の音声の設定が読まれて手伝い用のパネルに出ること', async () => {
    render(<HelperScreen />);
    await screen.findByTestId('room-code');

    fireEvent.click(screen.getByRole('button', { name: en.settings.title }));

    const panel = await screen.findByTestId('settings-help-panel');
    await waitFor(() => expect((panel.querySelector('#buffer-size') as HTMLSelectElement).value).toBe('64'));
    expect(askedOf('settings_get')).toHaveLength(1);
  });

  // Verifies: REQ-RMT-002
  it('パネルで設定を選んだとき、その 1 件が相手のアプリの settings_change として送られ、返った設定が出ること', async () => {
    render(<HelperScreen />);
    await screen.findByTestId('room-code');
    fireEvent.click(screen.getByRole('button', { name: en.settings.title }));
    const panel = await screen.findByTestId('settings-help-panel');

    fireEvent.change(panel.querySelector('#buffer-size')!, { target: { value: '128' } });

    await waitFor(() =>
      expect(askedOf('settings_change')).toEqual([
        { method: 'settings_change', params: { change: { setting: 'buffer_size', samples: 128 } } },
      ])
    );
    await waitFor(() => expect((panel.querySelector('#buffer-size') as HTMLSelectElement).value).toBe('128'));
  });

  // Verifies: REQ-RMT-002
  it('相手のアプリが設定を知らせてきたとき、パネルがそれに追いつき、古い知らせでは戻らないこと', async () => {
    render(<HelperScreen />);
    await screen.findByTestId('room-code');
    fireEvent.click(screen.getByRole('button', { name: en.settings.title }));
    const panel = await screen.findByTestId('settings-help-panel');
    await waitFor(() => expect((panel.querySelector('#buffer-size') as HTMLSelectElement).value).toBe('64'));

    hear('audio:config-changed', audioSettings({ revision: 5, buffer_size: 256 }));
    await waitFor(() => expect((panel.querySelector('#buffer-size') as HTMLSelectElement).value).toBe('256'));

    hear('audio:config-changed', audioSettings({ revision: 3, buffer_size: 32 }));
    expect((panel.querySelector('#buffer-size') as HTMLSelectElement).value).toBe('256');
  });

  // Verifies: REQ-RMT-002, REQ-RMT-006
  it('相手のアプリが設定を適用できなかったとき、決まった理由が利用者の言葉で出ること', async () => {
    onChange = () => new Error('device_gone');
    render(<HelperScreen />);
    await screen.findByTestId('room-code');
    fireEvent.click(screen.getByRole('button', { name: en.settings.title }));
    const panel = await screen.findByTestId('settings-help-panel');

    fireEvent.change(panel.querySelector('#buffer-size')!, { target: { value: '128' } });

    expect(await screen.findByTestId('settings-help-panel-status')).toHaveTextContent(
      en.settingsHelp.helper.refused.device_gone.replace('{{name}}', 'Bo')
    );
  });

  // Verifies: REQ-RMT-030
  it('相手のメーターの読み取りがまだ返らないとき、次の読み取りを頼まないこと', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let answer: (status: unknown) => void = () => {};
    const base = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'help_call' && (args as { method: string }).method === 'streaming_status') {
        asked.push({ method: 'streaming_status', params: {} });
        return new Promise((resolve) => (answer = resolve));
      }
      return base(cmd, args);
    });
    try {
      render(<HelperScreen />);
      await screen.findByTestId('room-code');
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000);
      });
      expect(askedOf('streaming_status')).toHaveLength(1);

      answer({ is_active: false });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(300);
      });
      expect(askedOf('streaming_status').length).toBeGreaterThan(1);
    } finally {
      vi.useRealTimers();
    }
  });

  // Verifies: REQ-RMT-030
  it('窓が見えていないとき、相手のメーターを読みに行かないこと', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    try {
      render(<HelperScreen />);
      await screen.findByTestId('room-code');
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1000);
      });

      expect(askedOf('streaming_status')).toHaveLength(0);

      visibility.mockReturnValue('visible');
      await act(async () => {
        await vi.advanceTimersByTimeAsync(300);
      });
      expect(askedOf('streaming_status').length).toBeGreaterThan(0);
    } finally {
      visibility.mockRestore();
      vi.useRealTimers();
    }
  });
});
