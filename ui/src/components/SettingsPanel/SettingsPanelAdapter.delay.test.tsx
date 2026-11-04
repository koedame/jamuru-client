/**
 * The device delay measurement in the user's own settings window: the Devices
 * tab shows the section, and pressing the button asks the app to measure.
 *
 * The measuring is the audio side's (`src/audio/delay_meter.rs`).
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from '../../i18n';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const idle = {
  running: false,
  session_running: true,
  stimulus: null,
  step: null,
  steps: 8,
  failure: null,
  figure: null,
  peers: {},
};

function fakeBackend() {
  const commands: string[] = [];
  invoke.mockImplementation(async (command: string) => {
    commands.push(command);
    switch (command) {
      case 'settings_get':
        return audioSettings({});
      case 'delay_measure_status':
        return idle;
      case 'config_get_peer_name':
        return 'Taro';
      default:
        return undefined;
    }
  });
  return commands;
}

describe('the device delay measurement in the settings panel', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-GUI-034
  it('the user opens the Devices tab, the measurement section is there', async () => {
    fakeBackend();
    render(<SettingsPanelAdapter initialTab="devices" />);

    expect(await screen.findByTestId('device-delay')).toBeInTheDocument();
  });

  // Verifies: REQ-GUI-034
  it('the user is in a room and presses the button, the app is asked to measure', async () => {
    const commands = fakeBackend();
    render(<SettingsPanelAdapter initialTab="devices" />);

    const start = await screen.findByTestId('device-delay-start');
    await waitFor(() => expect(start).toBeEnabled());
    fireEvent.click(start);

    await waitFor(() => expect(commands).toContain('delay_measure_start'));
  });
});
