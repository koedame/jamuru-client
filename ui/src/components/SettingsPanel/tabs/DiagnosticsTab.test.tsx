/**
 * DiagnosticsTab log file and report-a-problem sections (ADR-036, ADR-058)
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
        logFolderError="Could not open the log folder /logs/me.koeda.jamuru: No such file"
      />
    );

    expect(screen.getByRole('alert')).toHaveTextContent('/logs/me.koeda.jamuru');
  });

  // Verifies: REQ-GUI-020
  it('the folder was opened, its path is shown under the button', () => {
    render(
      <DiagnosticsTab state="idle" onOpenLogFolder={() => {}} logFolder="/logs/me.koeda.jamuru" />
    );

    expect(screen.getByText('/logs/me.koeda.jamuru')).toBeInTheDocument();
  });

  it('no handler is given, the section is not shown', () => {
    render(<DiagnosticsTab state="idle" />);

    expect(screen.queryByTestId('diagnostics-log-file')).not.toBeInTheDocument();
  });
});

describe('DiagnosticsTab report a problem section (ADR-058)', () => {
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
