/**
 * The settings window opens on the tab its address names and switches when the
 * app asks for another one (the menu bar's entries that live in a tab).
 */

import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';

import i18n from '../i18n';
import { SettingsScreen, settingsChecksUpdateFromHash, settingsTabFromHash } from './SettingsScreen';
import { audioSettings } from '../components/SettingsPanel/audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return Promise.resolve(() => listeners.delete(name));
  }),
}));

beforeAll(async () => {
  await i18n.changeLanguage('en');
});

beforeEach(() => {
  listeners.clear();
  invoke.mockReset();
  invoke.mockImplementation(async (command: string) => {
    switch (command) {
      case 'terms_get':
        return { version: 1, accepted: true, text: '# Terms' };
      case 'config_load':
        return { usage_reporting: false, buffer_size: 64 };
      case 'settings_get':
        return audioSettings();
      case 'config_get_peer_name':
        return 'Taro';
      case 'update_status':
        return { current: '0.1.0-128', support: 'install' };
      case 'update_check':
        return { kind: 'up_to_date' };
      case 'window_is_in_session':
        return false;
      default:
        return undefined;
    }
  });
});

describe('settingsTabFromHash', () => {
  // Verifies: REQ-MNU-007
  it('アドレスがタブを名指ししているとき、そのタブを返すこと', () => {
    expect(settingsTabFromHash('#/settings/about')).toBe('about');
    expect(settingsTabFromHash('#/settings/diagnostics')).toBe('diagnostics');
  });

  // Verifies: REQ-MNU-007
  it('タブを名指ししていないか、知らない名前のとき、null を返すこと', () => {
    expect(settingsTabFromHash('#/settings')).toBeNull();
    expect(settingsTabFromHash('#settings')).toBeNull();
    expect(settingsTabFromHash('#/settings/nowhere')).toBeNull();
  });
});

describe('SettingsScreen のタブの選び方', () => {
  // Verifies: REQ-MNU-007
  it('アドレスが「アプリについて」を名指ししているとき、そのタブで開くこと', async () => {
    window.location.hash = '#/settings/about';

    render(<SettingsScreen />);

    expect(await screen.findByTestId('settings-legal-terms')).toBeInTheDocument();
  });

  // Verifies: REQ-MNU-007
  it('開いているときにアプリからタブを替えるよう伝えられたとき、そのタブに替わること', async () => {
    window.location.hash = '#/settings';
    render(<SettingsScreen />);
    await screen.findByTestId('settings-panel');
    expect(screen.queryByTestId('settings-legal-terms')).not.toBeInTheDocument();

    act(() => listeners.get('settings:select-tab')!({ payload: 'about' }));

    expect(await screen.findByTestId('settings-legal-terms')).toBeInTheDocument();
  });

  // Verifies: REQ-MNU-007
  it('知らないタブの名前を伝えられたとき、今のタブのままであること', async () => {
    window.location.hash = '#/settings';
    render(<SettingsScreen />);
    await screen.findByTestId('settings-panel');

    act(() => listeners.get('settings:select-tab')!({ payload: 'nowhere' }));

    expect(screen.queryByTestId('settings-legal-terms')).not.toBeInTheDocument();
  });
});

describe('settingsChecksUpdateFromHash', () => {
  // Verifies: REQ-MNU-009
  it('アドレスが更新の確認を求めているとき、true を返すこと', () => {
    expect(settingsChecksUpdateFromHash('#/settings/about/check-update')).toBe(true);
  });

  // Verifies: REQ-MNU-009
  it('アドレスが更新の確認を求めていないとき、false を返すこと', () => {
    expect(settingsChecksUpdateFromHash('#/settings/about')).toBe(false);
    expect(settingsChecksUpdateFromHash('#/settings')).toBe(false);
    expect(settingsChecksUpdateFromHash('#/settings/about/other')).toBe(false);
  });

  // Verifies: REQ-MNU-009
  it('更新の確認を求めるアドレスでも、タブは「アプリについて」と読めること', () => {
    expect(settingsTabFromHash('#/settings/about/check-update')).toBe('about');
  });
});

describe('SettingsScreen の更新の確認', () => {
  // Verifies: REQ-MNU-009
  it('アドレスが更新の確認を求めているとき、開いてすぐ確認して結果を出すこと', async () => {
    window.location.hash = '#/settings/about/check-update';

    render(<SettingsScreen />);

    expect(await screen.findByText('You are on the latest version.')).toBeInTheDocument();
    expect(invoke.mock.calls.map((call) => call[0])).toContain('update_check');
  });

  // Verifies: REQ-MNU-009
  it('アドレスが更新の確認を求めていないとき、開いただけでは確認しないこと', async () => {
    window.location.hash = '#/settings/about';

    render(<SettingsScreen />);

    expect(await screen.findByTestId('settings-update-version')).toHaveTextContent('0.1.0-128');
    expect(invoke.mock.calls.map((call) => call[0])).not.toContain('update_check');
  });

  // Verifies: REQ-MNU-009
  it('開いているときにアプリから更新の確認を伝えられたとき、確認して結果を出すこと', async () => {
    window.location.hash = '#/settings/about';
    render(<SettingsScreen />);
    await screen.findByTestId('settings-update-version');

    act(() => listeners.get('settings:check-update')!({ payload: null }));

    expect(await screen.findByText('You are on the latest version.')).toBeInTheDocument();
  });
});
