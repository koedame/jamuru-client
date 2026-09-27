/**
 * "Report a problem" in the settings panel (ADR-058): a manual, one-off
 * send of jamjam.log and a comment, independent of usage reporting.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from '../../i18n';
import { SettingsPanelAdapter } from './SettingsPanelAdapter';
import { audioSettings } from './audioSettingsFixture';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

const LOG_TEXT = '[2026-09-27T12:00:00Z INFO jamjam] starting';

function fakeBackend(overrides: Record<string, (args?: unknown) => unknown> = {}) {
  const calls: { command: string; args: unknown }[] = [];
  invoke.mockImplementation(async (command: string, args?: unknown) => {
    calls.push({ command, args });
    if (command in overrides) return overrides[command](args);
    switch (command) {
      case 'config_load':
        return { usage_reporting: false, buffer_size: 64 };
      case 'settings_get':
        return audioSettings();
      case 'config_get_peer_name':
        return 'Taro';
      case 'report_problem_preview':
        return LOG_TEXT;
      case 'report_problem_send':
        return undefined;
      default:
        return undefined;
    }
  });
  return calls;
}

describe('report a problem in the settings panel', () => {
  beforeEach(async () => {
    invoke.mockReset();
    await i18n.changeLanguage('en');
  });

  it('starting the flow reads jamjam.log and shows exactly what would be sent, without touching usage reporting', async () => {
    const calls = fakeBackend();
    render(<SettingsPanelAdapter initialTab="diagnostics" />);

    fireEvent.click(await screen.findByTestId('diagnostics-report-problem-start'));

    expect(await screen.findByTestId('diagnostics-report-problem-preview')).toHaveTextContent(
      LOG_TEXT
    );
    expect(calls.some((c) => c.command === 'config_set_usage_reporting')).toBe(false);
  });

  it('pressing send submits the comment typed in, and nothing more is asked before sending', async () => {
    const calls = fakeBackend();
    render(<SettingsPanelAdapter initialTab="diagnostics" />);

    fireEvent.click(await screen.findByTestId('diagnostics-report-problem-start'));
    await screen.findByTestId('diagnostics-report-problem-preview');
    fireEvent.change(screen.getByTestId('diagnostics-report-problem-comment'), {
      target: { value: '音が届きません' },
    });
    fireEvent.click(screen.getByTestId('diagnostics-report-problem-send'));

    await waitFor(() =>
      expect(calls).toContainEqual({
        command: 'report_problem_send',
        args: { comment: '音が届きません' },
      })
    );
    expect(await screen.findByTestId('diagnostics-report-problem')).toHaveTextContent(
      'Sent. Thank you.'
    );
  });

  it('the send fails, the reason is shown and the comment is kept for a retry', async () => {
    fakeBackend({
      report_problem_send: () => {
        throw 'network error';
      },
    });
    render(<SettingsPanelAdapter initialTab="diagnostics" />);

    fireEvent.click(await screen.findByTestId('diagnostics-report-problem-start'));
    await screen.findByTestId('diagnostics-report-problem-preview');
    fireEvent.change(screen.getByTestId('diagnostics-report-problem-comment'), {
      target: { value: 'still here' },
    });
    fireEvent.click(screen.getByTestId('diagnostics-report-problem-send'));

    expect(await screen.findByRole('alert')).toHaveTextContent('network error');
    expect(screen.getByTestId('diagnostics-report-problem-comment')).toHaveValue('still here');
  });

  it('cancelling before sending does not call the send command', async () => {
    const calls = fakeBackend();
    render(<SettingsPanelAdapter initialTab="diagnostics" />);

    fireEvent.click(await screen.findByTestId('diagnostics-report-problem-start'));
    await screen.findByTestId('diagnostics-report-problem-preview');
    fireEvent.click(screen.getByTestId('diagnostics-report-problem-cancel'));

    expect(await screen.findByTestId('diagnostics-report-problem-start')).toBeInTheDocument();
    expect(calls.some((c) => c.command === 'report_problem_send')).toBe(false);
  });
});
