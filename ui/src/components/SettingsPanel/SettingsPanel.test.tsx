/**
 * SettingsPanel tabs: which item lives in which tab (ADR-080)
 */

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';
import { SettingsPanel, type SettingsPanelProps } from './SettingsPanel';

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, defaultValue?: string | object) =>
      typeof defaultValue === 'string' ? defaultValue : key,
  }),
}));

const noop = () => {};

const baseProps: SettingsPanelProps = {
  language: 'en',
  displayName: 'Taro',
  inputDevices: [],
  outputDevices: [],
  selectedInputId: null,
  selectedOutputId: null,
  onLanguageChange: noop,
  onDisplayNameChange: noop,
  onInputDeviceChange: noop,
  onOutputDeviceChange: noop,
  onUsageReportingChange: noop,
  onOpenLogFolder: noop,
  onOpenReportProblem: noop,
  onRunDiagnostics: noop,
  legal: {
    onOpenTerms: noop,
    onOpenLicense: noop,
    onOpenPrivacy: noop,
    onOpenAnnouncements: noop,
  },
};

function open(tab: string) {
  fireEvent.click(screen.getByRole('tab', { name: tab }));
}

describe('SettingsPanel tabs', () => {
  // Verifies: REQ-GUI-028
  it('the panel is shown, the tabs are General, Audio, Privacy, Troubleshooting and About in that order', () => {
    render(<SettingsPanel {...baseProps} />);

    expect(screen.getAllByRole('tab').map((tab) => tab.textContent)).toEqual([
      'General',
      'Audio',
      'Privacy',
      'Troubleshooting',
      'About',
    ]);
  });

  // Verifies: REQ-GUI-028
  it('the General tab is open, the display name, the language and the server are there and the rest is not', () => {
    render(<SettingsPanel {...baseProps} initialTab="general" />);

    expect(screen.getByLabelText('Display Name')).toHaveValue('Taro');
    expect(screen.getByLabelText('Language')).toBeInTheDocument();
    expect(document.getElementById('server-url-input')).not.toBeNull();
    expect(screen.queryByTestId('settings-legal')).not.toBeInTheDocument();
    expect(screen.queryByRole('switch')).not.toBeInTheDocument();
  });

  // Verifies: REQ-GUI-028
  it('the Privacy tab is open, usage reporting is there and the diagnostics are not', () => {
    render(<SettingsPanel {...baseProps} initialTab="privacy" />);

    expect(screen.getByRole('switch', { name: 'Send usage data' })).toBeInTheDocument();
    expect(screen.queryByTestId('diagnostics-open-log-folder')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Run Diagnostics' })).not.toBeInTheDocument();
  });

  // Verifies: REQ-GUI-028
  it('the Troubleshooting tab is open, the diagnostics, the log folder and the problem report are there and usage reporting is not', () => {
    render(<SettingsPanel {...baseProps} initialTab="diagnostics" />);

    expect(screen.getByRole('button', { name: 'Run Diagnostics' })).toBeInTheDocument();
    expect(screen.getByTestId('diagnostics-open-log-folder')).toBeInTheDocument();
    expect(screen.getByTestId('diagnostics-report-problem')).toBeInTheDocument();
    expect(screen.queryByRole('switch')).not.toBeInTheDocument();
  });

  // Verifies: REQ-GUI-028
  it('the About tab is chosen, the terms, the license, the privacy page and the announcements are there', () => {
    render(<SettingsPanel {...baseProps} />);

    open('About');

    for (const row of ['terms', 'license', 'privacy', 'announcements']) {
      expect(screen.getByTestId(`settings-legal-${row}`)).toBeInTheDocument();
    }
  });

  // Verifies: REQ-GUI-028
  it('the Audio tab is chosen, the devices and the sample rate and buffer size are there', () => {
    render(<SettingsPanel {...baseProps} />);

    open('Audio');

    expect(document.getElementById('input-device')).not.toBeNull();
    expect(document.getElementById('sample-rate')).not.toBeNull();
    expect(document.getElementById('buffer-size')).not.toBeNull();
  });

  // Verifies: REQ-GUI-028
  it('no rows for the terms are given, there is no About tab', () => {
    render(<SettingsPanel {...baseProps} legal={undefined} />);

    expect(screen.queryByRole('tab', { name: 'About' })).not.toBeInTheDocument();
  });
});
