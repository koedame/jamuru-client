/**
 * The transmit channel setting in the settings panel: choosing mono or stereo
 * is saved, and reaches a session that is already running.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from '../../i18n';
import en from '../../../locales/en.json';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

/** A stand-in for the app; `streaming` says whether a session is running. */
function fakeBackend({ streaming }: { streaming: boolean }) {
  const calls: { command: string; args: unknown }[] = [];
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    calls.push({ command, args });
    switch (command) {
      case 'config_load':
        return { buffer_size: 64 };
      case 'streaming_status':
        return { is_active: streaming };
      case 'audio_list_input_devices':
      case 'audio_list_output_devices':
        return [];
      case 'audio_get_current_devices':
        return { input_device_id: null, output_device_id: null };
      case 'audio_get_buffer_size':
        return 64;
      case 'config_get_peer_name':
        return 'Taro';
      case 'config_get_sample_rate':
        return 48000;
      case 'config_list_sample_rates':
        return [];
      case 'config_get_transmit_channels':
        return 2;
      case 'config_get_input_channels':
      case 'config_get_output_channels':
        return { channel_l: 1, channel_r: 2 };
      default:
        return undefined;
    }
  });
  return calls;
}

describe('the transmit channel setting in the settings panel', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-AUD-107
  it('the user picks mono during a session, the setting is saved and the session is told', async () => {
    const calls = fakeBackend({ streaming: true });
    render(<SettingsPanelAdapter initialTab="devices" />);

    fireEvent.click(await screen.findByRole('button', { name: en.settings.devices.mono }));

    await waitFor(() =>
      expect(calls).toContainEqual({ command: 'streaming_set_transmit_channels', args: { count: 1 } })
    );
    expect(calls).toContainEqual({ command: 'config_set_transmit_channels', args: { count: 1 } });
  });

  // Verifies: REQ-AUD-107
  it('the user picks mono with no session running, the setting is saved and no session is told', async () => {
    const calls = fakeBackend({ streaming: false });
    render(<SettingsPanelAdapter initialTab="devices" />);

    const mono = await screen.findByRole('button', { name: en.settings.devices.mono });
    const statusChecksBefore = calls.filter((c) => c.command === 'streaming_status').length;
    fireEvent.click(mono);

    await waitFor(() =>
      expect(calls.filter((c) => c.command === 'streaming_status').length).toBeGreaterThan(statusChecksBefore)
    );
    expect(calls).toContainEqual({ command: 'config_set_transmit_channels', args: { count: 1 } });
    expect(calls.some((c) => c.command === 'streaming_set_transmit_channels')).toBe(false);
  });
});
