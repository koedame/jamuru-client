/**
 * A Bluetooth device in the Devices tab: it is warned about, never refused.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from '../../i18n';
import en from '../../../locales/en.json';
import type { AudioDeviceInfo, AudioSettings } from '../../lib/tauri';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

function device(id: string, name: string, isBluetooth: boolean): AudioDeviceInfo {
  return {
    id,
    name,
    supported_sample_rates: [48000],
    supported_channels: [2],
    is_default: false,
    is_asio: false,
    is_bluetooth: isBluetooth,
  };
}

function fakeBackend(settings: AudioSettings) {
  const calls: { command: string; args: unknown }[] = [];
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    calls.push({ command, args });
    if (command === 'settings_get') return settings;
    if (command === 'settings_change') {
      const change = (args as { change: { setting: string; device_id: string } }).change;
      if (change.setting === 'input_device') {
        settings = { ...settings, input_device_id: change.device_id };
      }
      return settings;
    }
    if (command === 'config_get_peer_name') return 'Taro';
    return undefined;
  });
  return calls;
}

const devices = [device('wired', 'Scarlett 2i2', false), device('buds', "Taro's AirPods", true)];

describe('a Bluetooth device in the Devices tab', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-GUI-026
  it('a Bluetooth input is selected, a warning is shown under the input and not under the output', async () => {
    fakeBackend(
      audioSettings({
        input_devices: devices,
        output_devices: [device('wired', 'Scarlett 2i2', false)],
        input_device_id: 'buds',
        output_device_id: 'wired',
      })
    );
    render(<SettingsPanelAdapter initialTab="devices" />);

    const warnings = await screen.findAllByText(en.settings.devices.bluetoothWarning);
    expect(warnings).toHaveLength(1);
    expect(warnings[0].closest('.form-field')).toContainElement(document.getElementById('input-device'));
  });

  // Verifies: REQ-GUI-026
  it('a Bluetooth output is selected, a warning is shown under the output', async () => {
    fakeBackend(
      audioSettings({
        input_devices: [device('wired', 'Scarlett 2i2', false)],
        output_devices: devices,
        input_device_id: 'wired',
        output_device_id: 'buds',
      })
    );
    render(<SettingsPanelAdapter initialTab="devices" />);

    const warnings = await screen.findAllByText(en.settings.devices.bluetoothWarning);
    expect(warnings).toHaveLength(1);
    expect(warnings[0].closest('.form-field')).toContainElement(document.getElementById('output-device'));
  });

  // Verifies: REQ-GUI-026
  it('only wired devices are selected, no warning is shown', async () => {
    fakeBackend(
      audioSettings({ input_devices: devices, output_devices: devices, input_device_id: 'wired', output_device_id: 'wired' })
    );
    render(<SettingsPanelAdapter initialTab="devices" />);

    await screen.findByLabelText(en.settings.devices.inputDevice);
    expect(screen.queryByText(en.settings.devices.bluetoothWarning)).toBeNull();
  });

  // Verifies: REQ-GUI-026
  it('the list marks a Bluetooth device by its name', async () => {
    fakeBackend(audioSettings({ input_devices: devices, input_device_id: 'wired' }));
    render(<SettingsPanelAdapter initialTab="devices" />);

    expect(await screen.findByRole('option', { name: "Taro's AirPods (Bluetooth)" })).toBeInTheDocument();
  });

  // Verifies: REQ-GUI-026
  it('the user picks a Bluetooth device, the app is asked for it and the warning appears', async () => {
    const calls = fakeBackend(audioSettings({ input_devices: devices, input_device_id: 'wired' }));
    const { container } = render(<SettingsPanelAdapter initialTab="devices" />);
    await screen.findByRole('option', { name: "Taro's AirPods (Bluetooth)" });
    expect(screen.queryByText(en.settings.devices.bluetoothWarning)).toBeNull();

    fireEvent.change(container.querySelector('#input-device')!, { target: { value: 'buds' } });

    await waitFor(() =>
      expect(calls).toContainEqual({
        command: 'settings_change',
        args: { change: { setting: 'input_device', device_id: 'buds' } },
      })
    );
    expect(await screen.findByText(en.settings.devices.bluetoothWarning)).toBeInTheDocument();
  });
});
