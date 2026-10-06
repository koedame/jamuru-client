/**
 * The settings screen's rows for the terms of use, the license and the
 * published pages, against a backend that behaves like the app's: the terms
 * and the license are read from the app, the two pages are opened by it.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';

import i18n from '../../i18n';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

function fakeBackend() {
  const calls: { command: string; args: unknown }[] = [];
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    calls.push({ command, args });
    switch (command) {
      case 'terms_get':
        return { version: 1, accepted: true, text: '# 利用規約\n\n## 第 1 条（本サービスでできること）' };
      case 'terms_get_license':
        return 'jamuru Source Available License\n\n1. DEFINITIONS';
      case 'config_load':
        return { usage_reporting: false, buffer_size: 64 };
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

describe('the terms rows in the settings screen', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-TRM-004
  it('the general tab is shown, rows for the terms, the license, the privacy page and the announcements are there', async () => {
    fakeBackend();
    render(<SettingsPanelAdapter initialTab="general" />);

    expect(await screen.findByTestId('settings-legal-terms')).toHaveTextContent('Terms of Use');
    expect(screen.getByTestId('settings-legal-license')).toHaveTextContent('License');
    expect(screen.getByTestId('settings-legal-privacy')).toHaveTextContent('Privacy and security');
    expect(screen.getByTestId('settings-legal-announcements')).toHaveTextContent('Announcements');
  });

  // Verifies: REQ-TRM-004
  it('the terms row is pressed, the bundled text is read from the app and shown in a dialog that closes', async () => {
    fakeBackend();
    render(<SettingsPanelAdapter initialTab="general" />);

    fireEvent.click(await screen.findByTestId('settings-legal-terms'));

    const dialog = await screen.findByRole('dialog', { name: 'Terms of Use' });
    expect(dialog).toHaveTextContent('第 1 条（本サービスでできること）');
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  // Verifies: REQ-TRM-004
  it('the license row is pressed, the license is shown as it is written', async () => {
    fakeBackend();
    render(<SettingsPanelAdapter initialTab="general" />);

    fireEvent.click(await screen.findByTestId('settings-legal-license'));

    const dialog = await screen.findByRole('dialog', { name: 'License' });
    expect(dialog).toHaveTextContent('jamuru Source Available License');
  });

  // Verifies: REQ-TRM-004
  it('the privacy and announcements rows are pressed, the app is asked to open each named page and nothing else', async () => {
    const calls = fakeBackend();
    render(<SettingsPanelAdapter initialTab="general" />);

    fireEvent.click(await screen.findByTestId('settings-legal-privacy'));
    fireEvent.click(screen.getByTestId('settings-legal-announcements'));

    await waitFor(() =>
      expect(calls.filter((c) => c.command === 'terms_open_page')).toEqual([
        { command: 'terms_open_page', args: { page: 'privacy' } },
        { command: 'terms_open_page', args: { page: 'announcements' } },
      ])
    );
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});
