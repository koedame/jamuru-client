/**
 * The About tab's update section against a backend that behaves like the
 * app's: the version and what the build can do are read, a check asks the
 * app, and an install is asked of the app only for a build that installs.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { act, render, screen, fireEvent, waitFor } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';

import i18n from '../../i18n';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return Promise.resolve(() => listeners.delete(name));
  }),
}));

interface Backend {
  support: 'none' | 'notice' | 'install';
  checked: unknown;
  installed: unknown;
  inSession: boolean;
}

function fakeBackend(overrides: Partial<Backend> = {}) {
  const backend: Backend = {
    support: 'install',
    checked: { kind: 'up_to_date' },
    installed: { kind: 'restarting', version: '0.1.0-129' },
    inSession: false,
    ...overrides,
  };
  const calls: string[] = [];
  invoke.mockImplementation(async (command: string) => {
    calls.push(command);
    switch (command) {
      case 'update_status':
        return { current: '0.1.0-128', support: backend.support };
      case 'update_check':
        return backend.checked;
      case 'update_install':
        return backend.installed;
      case 'window_is_in_session':
        return backend.inSession;
      case 'terms_get':
        return { version: 1, accepted: true, text: '# Terms' };
      case 'config_load':
        return { usage_reporting: false, buffer_size: 64 };
      case 'settings_get':
        return audioSettings();
      case 'config_get_peer_name':
        return 'Taro';
      default:
        return undefined;
    }
  });
  return calls;
}

const available = { kind: 'available', version: '0.1.0-129' };

async function showUpdate(overrides: Partial<Backend> = {}) {
  const calls = fakeBackend(overrides);
  render(<SettingsPanelAdapter initialTab="about" />);
  await screen.findByTestId('settings-update-version');
  return calls;
}

describe('「アプリについて」タブのアップデート', () => {
  beforeEach(async () => {
    invoke.mockReset();
    listeners.clear();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-UPD-022
  it('タブを開いたとき、実行中の版を出し、確認はまだしないこと', async () => {
    const calls = await showUpdate();

    expect(screen.getByTestId('settings-update-version')).toHaveTextContent('Version 0.1.0-128');
    expect(screen.getByTestId('settings-update-check')).toHaveTextContent('Check for Updates');
    expect(calls).not.toContain('update_check');
  });

  // Verifies: REQ-UPD-022
  it('最新の版のとき、確認すると「最新の版」と出て、入れるボタンは出ないこと', async () => {
    await showUpdate();

    fireEvent.click(screen.getByTestId('settings-update-check'));

    expect(await screen.findByText('You are on the latest version.')).toBeInTheDocument();
    expect(screen.queryByTestId('settings-update-install')).not.toBeInTheDocument();
  });

  // Verifies: REQ-UPD-022
  it('確認しているあいだ、確認のボタンを押せないこと', async () => {
    await showUpdate({ checked: new Promise(() => {}) });

    fireEvent.click(screen.getByTestId('settings-update-check'));

    expect(await screen.findByText('Checking…')).toBeInTheDocument();
    expect(screen.getByTestId('settings-update-check')).toBeDisabled();
  });

  // Verifies: REQ-UPD-022
  it('開発用などで更新を確認しないビルドのとき、そう出し、ボタンを出さないこと', async () => {
    await showUpdate({ support: 'none' });

    expect(screen.getByTestId('settings-update-message')).toHaveTextContent(
      'This build does not check for updates'
    );
    expect(screen.queryByTestId('settings-update-check')).not.toBeInTheDocument();
  });

  // Verifies: REQ-UPD-022
  it('確認が失敗したとき、理由を出し、もう一度確認できること', async () => {
    const calls = await showUpdate();
    invoke.mockImplementationOnce(async () => {
      throw 'checking for a release did not finish in time';
    });

    fireEvent.click(screen.getByTestId('settings-update-check'));

    expect(await screen.findByText(/Could not update: checking for a release/)).toBeInTheDocument();
    expect(screen.getByTestId('settings-update-check')).toBeEnabled();
    expect(calls).toContain('update_status');
  });

  // Verifies: REQ-UPD-023
  it('自分で入れ替えられるビルドで新しい版が見つかったとき、版を出し、入れるボタンを出すこと', async () => {
    await showUpdate({ checked: available });

    fireEvent.click(screen.getByTestId('settings-update-check'));

    expect(await screen.findByText('A new version, 0.1.0-129, is out.')).toBeInTheDocument();
    expect(screen.getByTestId('settings-update-install')).toHaveTextContent('Update and Restart');
    expect(screen.getByTestId('settings-update-install')).toBeEnabled();
  });

  // Verifies: REQ-UPD-023
  it('入れるボタンを押したとき、アプリに入れるよう頼み、入れているあいだ押せないこと', async () => {
    const calls = await showUpdate({ checked: available });
    fireEvent.click(screen.getByTestId('settings-update-check'));
    fireEvent.click(await screen.findByTestId('settings-update-install'));

    expect(await screen.findByText(/Installing 0.1.0-129/)).toBeInTheDocument();
    expect(calls).toContain('update_install');
    expect(screen.getByTestId('settings-update-install')).toBeDisabled();
    expect(screen.getByTestId('settings-update-check')).toBeDisabled();
  });

  // Verifies: REQ-UPD-023
  it('自分で入れ替えられない配布形態のとき、入れ直すよう出し、入れるボタンは出さないこと', async () => {
    await showUpdate({ support: 'notice', checked: available });

    fireEvent.click(screen.getByTestId('settings-update-check'));

    expect(
      await screen.findByText('A new version, 0.1.0-129, is out. Please reinstall to update')
    ).toBeInTheDocument();
    expect(screen.queryByTestId('settings-update-install')).not.toBeInTheDocument();
  });

  // Verifies: REQ-UPD-024
  it('ルームの中のとき、入れるボタンを押せず、退室してからと伝えること', async () => {
    const calls = await showUpdate({ checked: available, inSession: true });
    await waitFor(() => expect(calls).toContain('window_is_in_session'));
    fireEvent.click(screen.getByTestId('settings-update-check'));

    expect(await screen.findByTestId('settings-update-in-session')).toHaveTextContent('Leave the room');
    expect(screen.getByTestId('settings-update-install')).toBeDisabled();
  });

  // Verifies: REQ-UPD-024
  it('ルームに入ったとき、押せなくなり、出たとき、また押せること', async () => {
    await showUpdate({ checked: available });
    fireEvent.click(screen.getByTestId('settings-update-check'));
    const install = await screen.findByTestId('settings-update-install');

    await waitFor(() => expect(listeners.has('session:connected')).toBe(true));
    act(() => listeners.get('session:connected')!({ payload: null }));
    await waitFor(() => expect(install).toBeDisabled());
    act(() => listeners.get('session:disconnected')!({ payload: null }));

    await waitFor(() => expect(install).toBeEnabled());
  });

  // Verifies: REQ-UPD-024
  it('入れようとしたときアプリがセッション中だと断ったとき、押せなくなり、断られた理由を伝えること', async () => {
    await showUpdate({ checked: available, installed: { kind: 'in_session' } });
    fireEvent.click(screen.getByTestId('settings-update-check'));
    fireEvent.click(await screen.findByTestId('settings-update-install'));

    expect(await screen.findByTestId('settings-update-in-session')).toBeInTheDocument();
    expect(screen.getByTestId('settings-update-install')).toBeDisabled();
  });

  // Verifies: REQ-UPD-023
  it('別の更新が進んでいるとアプリが答えたとき、しばらくしてからと伝えること', async () => {
    await showUpdate({ checked: available, installed: { kind: 'busy' } });
    fireEvent.click(screen.getByTestId('settings-update-check'));
    fireEvent.click(await screen.findByTestId('settings-update-install'));

    expect(await screen.findByText(/Another update is in progress/)).toBeInTheDocument();
  });

  // Verifies: REQ-UPD-023
  it('日本語のとき、版とボタンを日本語で出すこと', async () => {
    await i18n.changeLanguage('ja');
    await showUpdate({ checked: available });

    expect(screen.getByTestId('settings-update-version')).toHaveTextContent('現在の版 0.1.0-128');
    fireEvent.click(screen.getByTestId('settings-update-check'));

    expect(await screen.findByText('新しい版 0.1.0-129 が出ています。')).toBeInTheDocument();
    expect(screen.getByTestId('settings-update-install')).toHaveTextContent('更新して再起動');
  });
});
