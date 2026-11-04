/**
 * The switch for marking the audio packets as voice, in the settings panel's
 * General tab, against a backend that keeps `voice_priority` in the saved config.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from '../../i18n';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

function fakeBackend(saved: { voice_priority: boolean }, failToSave = false) {
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    switch (command) {
      case 'config_load':
        return { ...saved, buffer_size: 64 };
      case 'config_set_voice_priority':
        if (failToSave) throw new Error('disk full');
        saved.voice_priority = (args as { enabled: boolean }).enabled;
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

describe('the voice priority switch in the settings panel', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-CON-160
  it('the packets are marked, the switch is on', async () => {
    fakeBackend({ voice_priority: true });
    render(<SettingsPanelAdapter />);

    const toggle = await screen.findByTestId('general-voice-priority-toggle');

    expect(toggle).toBeChecked();
  });

  // Verifies: REQ-CON-160
  it('the marking was turned off, the switch is off', async () => {
    fakeBackend({ voice_priority: false });
    render(<SettingsPanelAdapter />);

    const toggle = await screen.findByTestId('general-voice-priority-toggle');

    await waitFor(() => expect(toggle).not.toBeChecked());
  });

  // Verifies: REQ-CON-160
  it('the switch is turned off, the setting is saved as off', async () => {
    const saved = { voice_priority: true };
    fakeBackend(saved);
    render(<SettingsPanelAdapter />);
    const toggle = await screen.findByTestId('general-voice-priority-toggle');
    await waitFor(() => expect(toggle).toBeChecked());

    fireEvent.click(toggle);

    await waitFor(() => expect(saved.voice_priority).toBe(false));
    expect(invoke.mock.calls.filter(([command]) => command === 'config_set_voice_priority')).toEqual([
      ['config_set_voice_priority', { enabled: false }, undefined],
    ]);
  });

  // Verifies: REQ-CON-160
  it('saving fails, the switch goes back to what is saved', async () => {
    fakeBackend({ voice_priority: true }, true);
    render(<SettingsPanelAdapter />);
    const toggle = await screen.findByTestId('general-voice-priority-toggle');

    fireEvent.click(toggle);

    await waitFor(() => expect(toggle).toBeChecked());
  });
});
