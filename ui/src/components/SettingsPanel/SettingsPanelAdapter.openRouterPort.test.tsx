/**
 * The switch for asking the router to open the audio port, in the settings panel's
 * General tab, against a backend that keeps `open_router_port` in the saved config.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from '../../i18n';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

function fakeBackend(saved: { open_router_port?: boolean }, failToSave = false) {
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    switch (command) {
      case 'config_load':
        return { ...saved, buffer_size: 64 };
      case 'config_set_open_router_port':
        if (failToSave) throw new Error('disk full');
        saved.open_router_port = (args as { enabled: boolean }).enabled;
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

describe('the open router port switch in the settings panel', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-CON-165
  it('the router is asked to open the port, the switch is on', async () => {
    fakeBackend({ open_router_port: true });
    render(<SettingsPanelAdapter />);

    const toggle = await screen.findByTestId('general-open-router-port-toggle');

    await waitFor(() => expect(toggle).toBeChecked());
  });

  // Verifies: REQ-CON-165
  it('the setting was never saved, the switch is on', async () => {
    fakeBackend({});
    render(<SettingsPanelAdapter />);

    const toggle = await screen.findByTestId('general-open-router-port-toggle');

    await waitFor(() => expect(toggle).toBeChecked());
  });

  // Verifies: REQ-CON-165
  it('the router is not asked, the switch is off', async () => {
    fakeBackend({ open_router_port: false });
    render(<SettingsPanelAdapter />);

    const toggle = await screen.findByTestId('general-open-router-port-toggle');

    await waitFor(() => expect(toggle).not.toBeChecked());
  });

  // Verifies: REQ-CON-165
  it('the switch is turned off, the setting is saved as off', async () => {
    const saved = { open_router_port: true };
    fakeBackend(saved);
    render(<SettingsPanelAdapter />);
    const toggle = await screen.findByTestId('general-open-router-port-toggle');
    await waitFor(() => expect(toggle).toBeChecked());

    fireEvent.click(toggle);

    await waitFor(() => expect(saved.open_router_port).toBe(false));
    expect(invoke.mock.calls.filter(([command]) => command === 'config_set_open_router_port')).toEqual([
      ['config_set_open_router_port', { enabled: false }, undefined],
    ]);
  });

  // Verifies: REQ-CON-165
  it('saving fails, the switch goes back to what is saved', async () => {
    fakeBackend({ open_router_port: true }, true);
    render(<SettingsPanelAdapter />);
    const toggle = await screen.findByTestId('general-open-router-port-toggle');
    await waitFor(() => expect(toggle).toBeChecked());

    fireEvent.click(toggle);

    await waitFor(() => expect(toggle).toBeChecked());
  });
});
