/**
 * The audio settings in the settings panel: a choice is handed to the app as
 * one change, the panel shows what the app says is now in effect, and a
 * change made from outside the panel (another window, a peer helping with
 * the settings) shows up without reopening it.
 *
 * Saving the change and telling a running session are the app's side
 * (`src-tauri/src/settings.rs`), verified there.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { act, render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from '../../i18n';
import en from '../../../locales/en.json';
import type { AudioSettings } from '../../lib/tauri';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

/** Handlers the panel registered, by event name, so a test can raise one. */
const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return Promise.resolve(() => listeners.delete(name));
  }),
}));

/** A stand-in for the app: it holds `settings` and applies changes to them. */
function fakeBackend(settings: AudioSettings) {
  const calls: { command: string; args: unknown }[] = [];
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    calls.push({ command, args });
    switch (command) {
      case 'settings_get':
        return settings;
      case 'settings_change': {
        const change = (args as { change: { setting: string; count?: number } }).change;
        if (change.setting === 'transmit_channels') {
          settings = { ...settings, transmit_channels: change.count! };
        }
        return settings;
      }
      case 'config_get_peer_name':
        return 'Taro';
      default:
        return undefined;
    }
  });
  return calls;
}

const monoButton = () => screen.findByRole('button', { name: en.settings.devices.mono });

describe('the audio settings in the settings panel', () => {
  beforeEach(async () => {
    invoke.mockReset();
    listeners.clear();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-AUD-107
  // Verifies: REQ-GUI-024
  it('the user picks mono, the app is asked for that one change and the panel shows mono', async () => {
    const calls = fakeBackend(audioSettings({ transmit_channels: 2 }));
    render(<SettingsPanelAdapter initialTab="devices" />);

    fireEvent.click(await monoButton());

    await waitFor(() =>
      expect(calls).toContainEqual({
        command: 'settings_change',
        args: { change: { setting: 'transmit_channels', count: 1 } },
      })
    );
    await waitFor(() => expect(screen.getByRole('button', { name: en.settings.devices.mono })).toHaveAttribute('aria-pressed', 'true'));
  });

  // Verifies: REQ-GUI-024
  it('the settings change elsewhere while the panel is open, the panel shows the new settings', async () => {
    fakeBackend(audioSettings({ transmit_channels: 2 }));
    render(<SettingsPanelAdapter initialTab="devices" />);
    const mono = await monoButton();
    await waitFor(() => expect(listeners.has('audio:config-changed')).toBe(true));
    expect(mono).toHaveAttribute('aria-pressed', 'false');

    act(() => listeners.get('audio:config-changed')!({ payload: audioSettings({ transmit_channels: 1 }) }));

    await waitFor(() => expect(screen.getByRole('button', { name: en.settings.devices.mono })).toHaveAttribute('aria-pressed', 'true'));
  });

  // The buffer sizes on offer are the app's: the panel once listed 8 and 16
  // samples of its own, which the app could not save.
  //
  // Verifies: REQ-GUI-024
  it('the panel offers exactly the buffer sizes the app reports', async () => {
    fakeBackend(audioSettings({ buffer_sizes: [64, 256], buffer_size: 64 }));
    const { container } = render(<SettingsPanelAdapter initialTab="devices" />);

    await monoButton();
    const offered = Array.from(container.querySelectorAll('#buffer-size option'))
      .filter((option) => !(option as HTMLOptionElement).disabled)
      .map((option) => option.getAttribute('value'));
    expect(offered).toEqual(['64', '256']);
  });

  // The answer to a change and the announcement of another can arrive in
  // either order; the newer settings stay on screen.
  //
  // Verifies: REQ-GUI-024
  it('settings older than the ones shown arrive, the panel keeps the newer ones', async () => {
    fakeBackend(audioSettings({ revision: 0, transmit_channels: 2 }));
    render(<SettingsPanelAdapter initialTab="devices" />);
    await monoButton();
    await waitFor(() => expect(listeners.has('audio:config-changed')).toBe(true));

    act(() => listeners.get('audio:config-changed')!({ payload: audioSettings({ revision: 5, transmit_channels: 1 }) }));
    act(() => listeners.get('audio:config-changed')!({ payload: audioSettings({ revision: 4, transmit_channels: 2 }) }));

    await waitFor(() => expect(screen.getByRole('button', { name: en.settings.devices.mono })).toHaveAttribute('aria-pressed', 'true'));
  });

  // A channel picker changes its own side only, so two quick changes cannot
  // undo each other; the right picker's empty choice is mono.
  //
  // Verifies: REQ-GUI-024
  it('the user picks none as the right input channel, the app is asked for mono on that side only', async () => {
    const calls = fakeBackend(audioSettings({ input_channels: { left: 1, right: 2 } }));
    const { container } = render(<SettingsPanelAdapter initialTab="devices" />);
    await monoButton();

    fireEvent.change(container.querySelector('#input-channel-r')!, { target: { value: '' } });

    await waitFor(() =>
      expect(calls).toContainEqual({
        command: 'settings_change',
        args: { change: { setting: 'input_channel', side: 'right', channel: null } },
      })
    );
  });

  // Verifies: REQ-GUI-024
  it('the input is mono, the right channel picker shows none', async () => {
    fakeBackend(audioSettings({ input_channels: { left: 3, right: null }, input_channel_count: 4 }));
    const { container } = render(<SettingsPanelAdapter initialTab="devices" />);
    await monoButton();

    await waitFor(() => expect((container.querySelector('#input-channel-r') as HTMLSelectElement).value).toBe(''));
    expect((container.querySelector('#input-channel-l') as HTMLSelectElement).value).toBe('3');
  });
});
