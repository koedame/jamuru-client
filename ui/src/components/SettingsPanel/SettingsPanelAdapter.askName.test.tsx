/**
 * The switch for the name check before entering a room, in the settings
 * panel's General tab, against a backend that keeps `ask_name_on_entry` in the
 * saved config.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from '../../i18n';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

function fakeBackend(saved: { ask_name_on_entry: boolean }, failToSave = false) {
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    switch (command) {
      case 'config_load':
        return { ...saved, buffer_size: 64 };
      case 'config_set_ask_name_on_entry':
        if (failToSave) throw new Error('disk full');
        saved.ask_name_on_entry = (args as { enabled: boolean }).enabled;
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

describe('the name check switch in the settings panel', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-CON-142
  it('the name is asked before entering, the switch is on', async () => {
    fakeBackend({ ask_name_on_entry: true });
    render(<SettingsPanelAdapter />);

    const toggle = await screen.findByTestId('general-ask-name-toggle');

    expect(toggle).toBeChecked();
  });

  // Verifies: REQ-CON-142
  it('the name check was stopped from the dialog, the switch is off', async () => {
    fakeBackend({ ask_name_on_entry: false });
    render(<SettingsPanelAdapter />);

    const toggle = await screen.findByTestId('general-ask-name-toggle');

    await waitFor(() => expect(toggle).not.toBeChecked());
  });

  // Verifies: REQ-CON-142
  it('the switch is turned back on, the setting is saved as on', async () => {
    const saved = { ask_name_on_entry: false };
    fakeBackend(saved);
    render(<SettingsPanelAdapter />);
    const toggle = await screen.findByTestId('general-ask-name-toggle');
    await waitFor(() => expect(toggle).not.toBeChecked());

    fireEvent.click(toggle);

    await waitFor(() => expect(saved.ask_name_on_entry).toBe(true));
    expect(invoke.mock.calls.filter(([command]) => command === 'config_set_ask_name_on_entry')).toEqual([
      ['config_set_ask_name_on_entry', { enabled: true }, undefined],
    ]);
  });

  // Verifies: REQ-CON-142
  it('saving fails, the switch goes back to what is saved', async () => {
    fakeBackend({ ask_name_on_entry: true }, true);
    render(<SettingsPanelAdapter />);
    const toggle = await screen.findByTestId('general-ask-name-toggle');

    fireEvent.click(toggle);

    await waitFor(() => expect(toggle).toBeChecked());
  });
});
