/**
 * The WASAPI mode in the Devices tab (Windows): every way of opening is
 * offered, each marked recommended or not for the devices in use.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';

import i18n from '../../i18n';
import en from '../../../locales/en.json';
import type { AudioSettings, WasapiMode, WasapiModeAdvice, WasapiSettings } from '../../lib/tauri';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

const text = en.settings.devices.wasapi;

function advice(
  mode: WasapiMode,
  verdict: WasapiModeAdvice['verdict'],
  reason: WasapiModeAdvice['reason'],
  periodUs: number | null
): WasapiModeAdvice {
  return { mode, verdict, reason, input_period_us: periodUs, output_period_us: periodUs };
}

/** A built-in codec: a 10 ms period shared, low latency no shorter, 3 ms exclusive */
const codec: WasapiSettings = {
  mode: 'shared',
  advice: {
    applies: true,
    modes: [
      advice('shared', 'not_recommended', 'faster_available', 10000),
      advice('low_latency', 'not_recommended', 'same_period', 10000),
      advice('exclusive', 'recommended', 'opens_exclusively', 3000),
    ],
  },
};

function fakeBackend(settings: AudioSettings) {
  const calls: { command: string; args: unknown }[] = [];
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    calls.push({ command, args });
    if (command === 'settings_get') return settings;
    if (command === 'settings_change') {
      const change = (args as { change: { setting: string; mode: WasapiMode } }).change;
      if (change.setting === 'wasapi_mode' && settings.wasapi) {
        settings = { ...settings, wasapi: { ...settings.wasapi, mode: change.mode } };
      }
      return settings;
    }
    if (command === 'config_get_peer_name') return 'Taro';
    return undefined;
  });
  return calls;
}

/** The card of one way of opening, by its name */
async function option(name: string) {
  const radio = await screen.findByRole('radio', { name: new RegExp(name.replace(/[()]/g, '\\$&')) });
  return { radio: radio as HTMLInputElement, card: radio.closest('label') as HTMLElement };
}

function latency(label: string): string {
  return screen.getByText(label).nextElementSibling?.textContent ?? '';
}

describe('the WASAPI mode in the Devices tab', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-GUI-031
  it('with a WASAPI device in use, all three ways are offered with their verdict, period and reason', async () => {
    fakeBackend(audioSettings({ wasapi: codec }));
    render(<SettingsPanelAdapter initialTab="devices" />);

    const shared = await option(text.modes.shared);
    expect(shared.radio.checked).toBe(true);
    expect(within(shared.card).getByText(text.verdict.not_recommended)).toBeInTheDocument();
    expect(within(shared.card).getByText('Period 10.0ms')).toBeInTheDocument();
    expect(within(shared.card).getByText(text.reason.faster_available)).toBeInTheDocument();

    const lowLatency = await option(text.modes.low_latency);
    expect(within(lowLatency.card).getByText(text.verdict.not_recommended)).toBeInTheDocument();
    expect(within(lowLatency.card).getByText(text.reason.same_period)).toBeInTheDocument();

    const exclusive = await option(text.modes.exclusive);
    expect(within(exclusive.card).getByText(text.verdict.recommended)).toBeInTheDocument();
    expect(within(exclusive.card).getByText('Period 3.0ms')).toBeInTheDocument();
    expect(within(exclusive.card).getByText(text.exclusiveCaution)).toBeInTheDocument();
  });

  // Verifies: REQ-GUI-031
  it('the caution about other apps is only on the exclusive mode', async () => {
    fakeBackend(audioSettings({ wasapi: codec }));
    render(<SettingsPanelAdapter initialTab="devices" />);

    await option(text.modes.exclusive);
    expect(screen.getAllByText(text.exclusiveCaution)).toHaveLength(1);
  });

  // Verifies: REQ-GUI-031
  it('the user picks the exclusive mode, the app is asked for it and it shows as chosen', async () => {
    const calls = fakeBackend(audioSettings({ wasapi: codec }));
    render(<SettingsPanelAdapter initialTab="devices" />);

    fireEvent.click((await option(text.modes.exclusive)).radio);

    await waitFor(() =>
      expect(calls).toContainEqual({
        command: 'settings_change',
        args: { change: { setting: 'wasapi_mode', mode: 'exclusive' } },
      })
    );
    await waitFor(async () => expect((await option(text.modes.exclusive)).radio.checked).toBe(true));
  });

  // Verifies: REQ-GUI-031
  it('the latency shown is the period the chosen way runs the devices at, when longer than the buffer', async () => {
    fakeBackend(audioSettings({ buffer_size: 64, sample_rate: 48000, wasapi: codec }));
    const { unmount } = render(<SettingsPanelAdapter initialTab="devices" />);
    await option(text.modes.shared);
    expect(latency(en.settings.devices.latencyInput)).toBe('10.0ms');
    expect(latency(en.settings.devices.latencyTotal)).toBe('20.0ms');
    unmount();

    fakeBackend(audioSettings({ buffer_size: 64, sample_rate: 48000, wasapi: { ...codec, mode: 'exclusive' } }));
    render(<SettingsPanelAdapter initialTab="devices" />);
    await option(text.modes.exclusive);
    expect(latency(en.settings.devices.latencyInput)).toBe('3.0ms');
    expect(latency(en.settings.devices.latencyTotal)).toBe('6.0ms');
  });

  // Verifies: REQ-GUI-031
  it('a way that does not open the devices cannot be chosen, and says why', async () => {
    fakeBackend(
      audioSettings({
        wasapi: {
          mode: 'shared',
          advice: {
            applies: true,
            modes: [
              advice('shared', 'recommended', 'nothing_faster', 10000),
              advice('low_latency', 'unavailable', 'no_low_latency_mode', null),
              advice('exclusive', 'unavailable', 'no_exclusive_format', null),
            ],
          },
        },
      })
    );
    render(<SettingsPanelAdapter initialTab="devices" />);

    const exclusive = await option(text.modes.exclusive);
    expect(exclusive.radio.disabled).toBe(true);
    expect(within(exclusive.card).getByText(text.verdict.unavailable)).toBeInTheDocument();
    expect(within(exclusive.card).getByText(text.reason.no_exclusive_format)).toBeInTheDocument();
    expect(within(exclusive.card).queryByText(text.exclusiveCaution)).toBeNull();
    expect((await option(text.modes.low_latency)).radio.disabled).toBe(true);
    expect((await option(text.modes.shared)).radio.disabled).toBe(false);
  });

  // Verifies: REQ-GUI-031
  it('a saved way the devices now in use do not open stays shown as chosen, and the latency is the standard way', async () => {
    fakeBackend(
      audioSettings({
        wasapi: {
          mode: 'exclusive',
          advice: {
            applies: true,
            modes: [
              advice('shared', 'recommended', 'nothing_faster', 10000),
              advice('low_latency', 'not_recommended', 'same_period', 10000),
              advice('exclusive', 'unavailable', 'no_exclusive_format', null),
            ],
          },
        },
      })
    );
    render(<SettingsPanelAdapter initialTab="devices" />);

    expect((await option(text.modes.exclusive)).radio.checked).toBe(true);
    expect(latency(en.settings.devices.latencyInput)).toBe('10.0ms');
  });

  // Verifies: REQ-GUI-031
  it('a saved exclusive mode that only one device opens shows that device at the exclusive period and keeps the caution', async () => {
    fakeBackend(
      audioSettings({
        buffer_size: 64,
        sample_rate: 48000,
        wasapi: {
          mode: 'exclusive',
          advice: {
            applies: true,
            modes: [
              advice('shared', 'recommended', 'nothing_faster', 10000),
              advice('low_latency', 'not_recommended', 'same_period', 10000),
              {
                mode: 'exclusive',
                verdict: 'unavailable',
                reason: 'no_exclusive_format',
                input_period_us: null,
                output_period_us: 3000,
              },
            ],
          },
        },
      })
    );
    render(<SettingsPanelAdapter initialTab="devices" />);

    const exclusive = await option(text.modes.exclusive);
    expect(exclusive.radio.checked).toBe(true);
    expect(within(exclusive.card).getByText(text.exclusiveCaution)).toBeInTheDocument();
    expect(latency(en.settings.devices.latencyInput)).toBe('10.0ms');
    expect(latency(en.settings.devices.latencyOutput)).toBe('3.0ms');
  });

  // Verifies: REQ-GUI-031
  it('with an ASIO driver on both sides, the choice is replaced by a note that it is not used', async () => {
    fakeBackend(audioSettings({ wasapi: { mode: 'exclusive', advice: { applies: false, modes: [] } } }));
    render(<SettingsPanelAdapter initialTab="devices" />);

    expect(await screen.findByText(text.notApplied)).toBeInTheDocument();
    expect(screen.queryByRole('radio')).toBeNull();
    expect(latency(en.settings.devices.latencyInput)).toBe('1.3ms');
  });

  // Verifies: REQ-GUI-031
  it('where no device is a WASAPI one (not Windows), there is no WASAPI mode at all', async () => {
    fakeBackend(audioSettings());
    render(<SettingsPanelAdapter initialTab="devices" />);

    await screen.findByLabelText(en.settings.devices.inputDevice);
    expect(screen.queryByText(text.label)).toBeNull();
    expect(screen.queryByRole('radio')).toBeNull();
  });
});
