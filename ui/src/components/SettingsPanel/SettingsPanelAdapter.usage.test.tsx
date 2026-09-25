/**
 * Usage reporting in the settings panel, against a backend that behaves like
 * the app's: the switch sets the one setting with `config_set_usage_reporting`,
 * and `usage_preview` returns nothing while it is off.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from '../../i18n';
import en from '../../../locales/en.json';
import ja from '../../../locales/ja.json';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

const INSTALL_LINE =
  '{"v":1,"seq":1,"event":"app_start","install_id":"a91d5c0e7b3f4a68b2c1d0e9f8a7b6c5"}';

/** A stand-in for the app: `usage_reporting` lives in the saved config. */
function fakeBackend(saved: { usage_reporting: boolean }) {
  const calls: { command: string; args: unknown }[] = [];
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    calls.push({ command, args });
    switch (command) {
      case 'config_load':
        return { ...saved, buffer_size: 64 };
      case 'config_set_usage_reporting':
        saved.usage_reporting = (args as { enabled: boolean }).enabled;
        return undefined;
      case 'usage_preview':
        return saved.usage_reporting ? INSTALL_LINE : '';
      case 'settings_get':
        return audioSettings();
      case 'config_get_peer_name':
        return 'Taro';
      default:
        return undefined;
    }
  });
  return calls;
}

describe('the usage reporting switch in the settings panel', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-TEL-011
  it('the app is opened for the first time, the switch is off and no dialog asks about it', async () => {
    fakeBackend({ usage_reporting: false });
    render(<SettingsPanelAdapter initialTab="diagnostics" />);

    const toggle = await screen.findByRole('switch', { name: en.settings.diagnostics.usageToggle });

    expect(toggle).not.toBeChecked();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  });

  // Verifies: REQ-TEL-011
  it('the setting is saved on, the switch starts on', async () => {
    fakeBackend({ usage_reporting: true });
    render(<SettingsPanelAdapter initialTab="diagnostics" />);

    const toggle = await screen.findByRole('switch', { name: en.settings.diagnostics.usageToggle });

    await waitFor(() => expect(toggle).toBeChecked());
  });

  // Verifies: REQ-TEL-011
  it('the user turns the switch on, the setting is saved as on and nothing else in the config changes', async () => {
    const saved = { usage_reporting: false };
    const calls = fakeBackend(saved);
    render(<SettingsPanelAdapter initialTab="diagnostics" />);

    fireEvent.click(await screen.findByRole('switch', { name: en.settings.diagnostics.usageToggle }));

    await waitFor(() => expect(saved.usage_reporting).toBe(true));
    // Only the switch's own setting travels: nothing read earlier is written
    // back over a change made meanwhile.
    expect(calls).toContainEqual({ command: 'config_set_usage_reporting', args: { enabled: true } });
  });

  // Verifies: REQ-TEL-011
  it('saving fails, the switch goes back and the reason is shown', async () => {
    fakeBackend({ usage_reporting: false });
    const backend = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'config_set_usage_reporting') throw 'disk full';
      return backend(command, args);
    });
    render(<SettingsPanelAdapter initialTab="diagnostics" />);

    const toggle = await screen.findByRole('switch', { name: en.settings.diagnostics.usageToggle });
    fireEvent.click(toggle);

    expect(await screen.findByRole('alert')).toHaveTextContent('disk full');
    await waitFor(() => expect(toggle).not.toBeChecked());
  });

  // Verifies: REQ-TEL-013
  it('the user turns it on and asks what is sent, the lines with the install ID are shown', async () => {
    const saved = { usage_reporting: false };
    fakeBackend(saved);
    render(<SettingsPanelAdapter initialTab="diagnostics" />);

    fireEvent.click(await screen.findByRole('switch', { name: en.settings.diagnostics.usageToggle }));
    await waitFor(() => expect(saved.usage_reporting).toBe(true));
    fireEvent.click(screen.getByRole('button', { name: en.settings.diagnostics.usageShow }));

    expect(await screen.findByTestId('diagnostics-usage-preview')).toHaveTextContent(INSTALL_LINE);
  });

  // Verifies: REQ-TEL-013
  it('the user turns it off while the lines are shown, they are replaced by a note that nothing is sent and there is no ID', async () => {
    fakeBackend({ usage_reporting: true });
    render(<SettingsPanelAdapter initialTab="diagnostics" />);

    const toggle = await screen.findByRole('switch', { name: en.settings.diagnostics.usageToggle });
    await waitFor(() => expect(toggle).toBeChecked());
    fireEvent.click(screen.getByRole('button', { name: en.settings.diagnostics.usageShow }));
    expect(await screen.findByTestId('diagnostics-usage-preview')).toHaveTextContent(INSTALL_LINE);

    fireEvent.click(toggle);

    await waitFor(() =>
      expect(screen.getByTestId('diagnostics-usage-preview')).toHaveTextContent(
        en.settings.diagnostics.usagePreviewOff
      )
    );
    expect(screen.getByTestId('diagnostics-usage-preview')).not.toHaveTextContent('install_id');
  });

  // Verifies: REQ-TEL-012
  it.each([
    ['en', en],
    ['ja', ja],
  ])('the language is %s, the section reads in that language', async (language, bundle) => {
    fakeBackend({ usage_reporting: false });
    await i18n.changeLanguage(language);
    render(<SettingsPanelAdapter initialTab="diagnostics" />);

    await screen.findByRole('switch', { name: bundle.settings.diagnostics.usageToggle });

    const section = screen.getByTestId('diagnostics-usage');
    expect(section).toHaveTextContent(bundle.settings.diagnostics.usageSent);
    expect(section).toHaveTextContent(bundle.settings.diagnostics.usageNotSent);
    expect(section).toHaveTextContent(bundle.settings.diagnostics.usageDeviceNames);
    expect(section).toHaveTextContent(bundle.settings.diagnostics.usageIdNote);
  });
});
