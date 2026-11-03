/**
 * The limiter switches in the settings panel: a switch is handed to the app as
 * one change, and the panel shows what the app says is now in effect.
 *
 * Holding the audio at 0 dBFS is the audio side's (`src/audio/limiter.rs`).
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from '../../i18n';
import type { AudioSettings } from '../../lib/tauri';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

/** A stand-in for the app: it holds `settings` and applies the limiter changes to them. */
function fakeBackend(settings: AudioSettings) {
  const calls: { command: string; args: unknown }[] = [];
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    calls.push({ command, args });
    switch (command) {
      case 'settings_get':
        return settings;
      case 'settings_change': {
        const change = (args as { change: { setting: string; enabled?: boolean } }).change;
        if (change.setting === 'input_limiter') settings = { ...settings, input_limiter: change.enabled! };
        if (change.setting === 'output_limiter') settings = { ...settings, output_limiter: change.enabled! };
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

describe('the limiter switches in the settings panel', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-GUI-032
  it('the limiters are on, the switches show both on', async () => {
    fakeBackend(audioSettings({ input_limiter: true, output_limiter: true }));
    render(<SettingsPanelAdapter initialTab="devices" />);

    expect(await screen.findByTestId('devices-input-limiter-toggle')).toBeChecked();
    expect(screen.getByTestId('devices-output-limiter-toggle')).toBeChecked();
  });

  // Verifies: REQ-GUI-032
  it('the user switches the input limiter off, the app is asked for that one change and the panel shows it off', async () => {
    const calls = fakeBackend(audioSettings({ input_limiter: true, output_limiter: true }));
    render(<SettingsPanelAdapter initialTab="devices" />);

    fireEvent.click(await screen.findByTestId('devices-input-limiter-toggle'));

    await waitFor(() =>
      expect(calls).toContainEqual({
        command: 'settings_change',
        args: { change: { setting: 'input_limiter', enabled: false } },
      })
    );
    await waitFor(() => expect(screen.getByTestId('devices-input-limiter-toggle')).not.toBeChecked());
    expect(screen.getByTestId('devices-output-limiter-toggle')).toBeChecked();
  });

  // Verifies: REQ-GUI-032
  it('the user switches the output limiter off, the app is asked for that one change and the panel shows it off', async () => {
    const calls = fakeBackend(audioSettings({ input_limiter: true, output_limiter: true }));
    render(<SettingsPanelAdapter initialTab="devices" />);

    fireEvent.click(await screen.findByTestId('devices-output-limiter-toggle'));

    await waitFor(() =>
      expect(calls).toContainEqual({
        command: 'settings_change',
        args: { change: { setting: 'output_limiter', enabled: false } },
      })
    );
    await waitFor(() => expect(screen.getByTestId('devices-output-limiter-toggle')).not.toBeChecked());
    expect(screen.getByTestId('devices-input-limiter-toggle')).toBeChecked();
  });
});
