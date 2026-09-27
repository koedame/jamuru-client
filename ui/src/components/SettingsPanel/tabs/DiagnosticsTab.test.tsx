/**
 * DiagnosticsTab log file section (ADR-036)
 */

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';
import { DiagnosticsTab } from './DiagnosticsTab';

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (_key: string, defaultValue?: string) => defaultValue ?? _key,
  }),
}));

describe('DiagnosticsTab log file section', () => {
  // Verifies: REQ-GUI-020
  it('the user presses the button, the handler runs', () => {
    const onOpenLogFolder = vi.fn();
    render(<DiagnosticsTab state="idle" onOpenLogFolder={onOpenLogFolder} />);

    fireEvent.click(screen.getByRole('button', { name: 'Open log folder' }));

    expect(onOpenLogFolder).toHaveBeenCalledTimes(1);
  });

  // Verifies: REQ-GUI-020
  it('the folder could not be opened, the reason with its path is shown as an alert', () => {
    render(
      <DiagnosticsTab
        state="idle"
        onOpenLogFolder={() => {}}
        logFolderError="Could not open the log folder /logs/me.koeda.jamjam: No such file"
      />
    );

    expect(screen.getByRole('alert')).toHaveTextContent('/logs/me.koeda.jamjam');
  });

  // Verifies: REQ-GUI-020
  it('the folder was opened, its path is shown under the button', () => {
    render(
      <DiagnosticsTab state="idle" onOpenLogFolder={() => {}} logFolder="/logs/me.koeda.jamjam" />
    );

    expect(screen.getByText('/logs/me.koeda.jamjam')).toBeInTheDocument();
  });

  it('no handler is given, the section is not shown', () => {
    render(<DiagnosticsTab state="idle" />);

    expect(screen.queryByTestId('diagnostics-log-file')).not.toBeInTheDocument();
  });
});

const INSTALL_LINE =
  '{"v":1,"ts":"2026-09-24T02:10:00Z","seq":1,"event":"app_start","install_id":"a91d5c0e7b3f4a68b2c1d0e9f8a7b6c5","launch_id":"7f3c"}';

describe('DiagnosticsTab usage reporting section', () => {
  // Verifies: REQ-TEL-011
  it('usage reporting is not on, the switch is off and nothing is previewed', () => {
    render(<DiagnosticsTab state="idle" onUsageReportingChange={() => {}} />);

    expect(screen.getByRole('switch', { name: 'Send usage data' })).not.toBeChecked();
    expect(screen.queryByTestId('diagnostics-usage-preview')).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  // Verifies: REQ-TEL-011
  it('the user turns the switch on, the handler gets true', () => {
    const onUsageReportingChange = vi.fn();
    render(<DiagnosticsTab state="idle" onUsageReportingChange={onUsageReportingChange} />);

    fireEvent.click(screen.getByRole('switch', { name: 'Send usage data' }));

    expect(onUsageReportingChange).toHaveBeenCalledWith(true);
  });

  // Verifies: REQ-TEL-011
  it('usage reporting is on, the switch is on and turning it off gives false', () => {
    const onUsageReportingChange = vi.fn();
    render(
      <DiagnosticsTab state="idle" usageReporting onUsageReportingChange={onUsageReportingChange} />
    );

    const toggle = screen.getByRole('switch', { name: 'Send usage data' });
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);

    expect(onUsageReportingChange).toHaveBeenCalledWith(false);
  });

  // Verifies: REQ-TEL-011
  it('no handler is given, the section is not shown', () => {
    render(<DiagnosticsTab state="idle" />);

    expect(screen.queryByTestId('diagnostics-usage')).not.toBeInTheDocument();
  });

  // Verifies: REQ-TEL-012
  it('the section is shown, it says what is sent, what is not, and that device names can hold a name', () => {
    render(<DiagnosticsTab state="idle" onUsageReportingChange={() => {}} />);

    const section = screen.getByTestId('diagnostics-usage');
    expect(section).toHaveTextContent('Sent: app version, OS, CPU and memory, audio device names');
    expect(section).toHaveTextContent('your own IP addresses');
    expect(section).toHaveTextContent(
      'Never sent: display name, room history, machine identifier, anything about the other people in the room (their IP addresses included), audio, chat.'
    );
    expect(section).toHaveTextContent("Taro's AirPods");
    expect(section).toHaveTextContent('discards the ID');
  });

  // Verifies: REQ-TEL-013
  it('the user asks what is sent, the handler runs', () => {
    const onShowUsagePreview = vi.fn();
    render(
      <DiagnosticsTab
        state="idle"
        onUsageReportingChange={() => {}}
        onShowUsagePreview={onShowUsagePreview}
      />
    );

    fireEvent.click(screen.getByRole('button', { name: 'Show what is sent' }));

    expect(onShowUsagePreview).toHaveBeenCalledTimes(1);
  });

  // Verifies: REQ-TEL-013
  it('lines are waiting to be sent, they are shown as they are, install ID included', () => {
    render(
      <DiagnosticsTab
        state="idle"
        usageReporting
        onUsageReportingChange={() => {}}
        usagePreview={INSTALL_LINE}
      />
    );

    expect(screen.getByTestId('diagnostics-usage-preview')).toHaveTextContent(INSTALL_LINE);
  });

  // Verifies: REQ-TEL-013
  it('usage reporting is off and the preview is empty, it says nothing is sent and there is no ID', () => {
    render(
      <DiagnosticsTab state="idle" onUsageReportingChange={() => {}} usagePreview="" />
    );

    expect(screen.getByTestId('diagnostics-usage-preview')).toHaveTextContent(
      'Off: nothing is collected or sent, and there is no install ID.'
    );
  });

  // Verifies: REQ-TEL-013
  it('usage reporting is on and nothing is waiting, it says so', () => {
    render(
      <DiagnosticsTab state="idle" usageReporting onUsageReportingChange={() => {}} usagePreview="" />
    );

    expect(screen.getByTestId('diagnostics-usage-preview')).toHaveTextContent(
      'Nothing is waiting to be sent yet.'
    );
  });

  it('the preview could not be read, the reason is shown as an alert', () => {
    render(
      <DiagnosticsTab
        state="idle"
        onUsageReportingChange={() => {}}
        usagePreviewError="usage_preview failed"
      />
    );

    expect(screen.getByRole('alert')).toHaveTextContent('usage_preview failed');
  });
});

describe('DiagnosticsTab report a problem section (ADR-057)', () => {
  // Verifies: REQ-RPT-001
  it('nothing is rendered when there is no handler to start it', () => {
    render(<DiagnosticsTab state="idle" />);

    expect(screen.queryByTestId('diagnostics-report-problem')).not.toBeInTheDocument();
  });

  // Verifies: REQ-RPT-001
  it('idle state shows a button that starts the flow', () => {
    const onOpenReportProblem = vi.fn();
    render(<DiagnosticsTab state="idle" onOpenReportProblem={onOpenReportProblem} />);

    fireEvent.click(screen.getByTestId('diagnostics-report-problem-start'));

    expect(onOpenReportProblem).toHaveBeenCalledTimes(1);
  });

  // Verifies: REQ-RPT-002
  it('ready state shows exactly the preview text that would be sent', () => {
    render(
      <DiagnosticsTab
        state="idle"
        onOpenReportProblem={() => {}}
        reportProblemState="ready"
        reportProblemPreview={"line one\nline two"}
      />
    );

    expect(screen.getByTestId('diagnostics-report-problem-preview')).toHaveTextContent(
      'line one line two'
    );
  });

  // Verifies: REQ-RPT-001
  it('the comment field is capped at 2000 characters', () => {
    render(
      <DiagnosticsTab state="idle" onOpenReportProblem={() => {}} reportProblemState="ready" />
    );

    expect(screen.getByTestId('diagnostics-report-problem-comment')).toHaveAttribute(
      'maxlength',
      '2000'
    );
  });

  // Verifies: REQ-RPT-001
  it('pressing send calls the handler with no separate consent step', () => {
    const onSendReportProblem = vi.fn();
    render(
      <DiagnosticsTab
        state="idle"
        onOpenReportProblem={() => {}}
        reportProblemState="ready"
        onSendReportProblem={onSendReportProblem}
      />
    );

    fireEvent.click(screen.getByTestId('diagnostics-report-problem-send'));

    expect(onSendReportProblem).toHaveBeenCalledTimes(1);
  });

  it('sending state disables the send and cancel buttons', () => {
    render(
      <DiagnosticsTab
        state="idle"
        onOpenReportProblem={() => {}}
        reportProblemState="sending"
        onSendReportProblem={() => {}}
        onCancelReportProblem={() => {}}
      />
    );

    expect(screen.getByTestId('diagnostics-report-problem-send')).toBeDisabled();
    expect(screen.getByTestId('diagnostics-report-problem-cancel')).toBeDisabled();
  });

  it('sent state shows a confirmation instead of the form', () => {
    render(
      <DiagnosticsTab
        state="idle"
        onOpenReportProblem={() => {}}
        reportProblemState="sent"
        onCancelReportProblem={() => {}}
      />
    );

    expect(screen.queryByTestId('diagnostics-report-problem-comment')).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId('diagnostics-report-problem-close'));
  });

  it('error state shows the reason as an alert and offers a retry', () => {
    render(
      <DiagnosticsTab
        state="idle"
        onOpenReportProblem={() => {}}
        reportProblemState="error"
        reportProblemError="送信できませんでした"
        onSendReportProblem={() => {}}
      />
    );

    expect(screen.getByRole('alert')).toHaveTextContent('送信できませんでした');
    expect(screen.getByTestId('diagnostics-report-problem-send')).toHaveTextContent('Try again');
  });
});
