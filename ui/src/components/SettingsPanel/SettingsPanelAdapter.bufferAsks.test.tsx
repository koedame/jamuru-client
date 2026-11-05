/**
 * Whether asks to buffer a participant's audio for longer are put to the user
 * or turned down (ADR-097), in the settings panel's General tab, against a
 * backend that keeps `buffer_asks` in the saved config.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from '../../i18n';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

function fakeBackend(saved: { buffer_asks: string }, failToSave = false) {
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    switch (command) {
      case 'config_load':
        return { ...saved, buffer_size: 64 };
      case 'buffer_request_set_policy':
        if (failToSave) throw new Error('disk full');
        saved.buffer_asks = (args as { policy: string }).policy;
        return undefined;
      case 'settings_get':
        return audioSettings();
      case 'config_get_peer_name':
        return 'Taro';
      default:
        return undefined;
    }
  });
}

describe('the buffer asks setting in the settings panel', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-GUI-036
  it('the saved setting asks each time, Ask me each time is the choice', async () => {
    fakeBackend({ buffer_asks: 'ask' });
    render(<SettingsPanelAdapter />);

    const select = await screen.findByLabelText(/When someone asks me to buffer/);
    await waitFor(() => expect(select).toHaveValue('ask'));
  });

  // Verifies: REQ-GUI-036
  it('the saved setting takes no asks, Do not take asks is the choice', async () => {
    fakeBackend({ buffer_asks: 'never' });
    render(<SettingsPanelAdapter />);

    const select = await screen.findByLabelText(/When someone asks me to buffer/);
    await waitFor(() => expect(select).toHaveValue('never'));
  });

  // Verifies: REQ-GUI-036
  it('Do not take asks is chosen, the backend is told and keeps it', async () => {
    const saved = { buffer_asks: 'ask' };
    fakeBackend(saved);
    render(<SettingsPanelAdapter />);

    const select = await screen.findByLabelText(/When someone asks me to buffer/);
    fireEvent.change(select, { target: { value: 'never' } });

    await waitFor(() => expect(saved.buffer_asks).toBe('never'));
  });

  // Verifies: REQ-GUI-036
  it('the backend cannot save the choice, the choice goes back to what it was', async () => {
    fakeBackend({ buffer_asks: 'ask' }, true);
    vi.spyOn(console, 'error').mockImplementation(() => {});
    render(<SettingsPanelAdapter />);

    const select = await screen.findByLabelText(/When someone asks me to buffer/);
    fireEvent.change(select, { target: { value: 'never' } });

    await waitFor(() => expect(select).toHaveValue('ask'));
  });
});
