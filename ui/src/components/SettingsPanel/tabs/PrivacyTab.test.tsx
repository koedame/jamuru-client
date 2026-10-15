/**
 * PrivacyTab usage reporting (ADR-037) and previous hang (ADR-056, ADR-059) sections
 */

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';
import { PrivacyTab } from './PrivacyTab';

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (_key: string, defaultValue?: string) => defaultValue ?? _key,
  }),
}));

const INSTALL_LINE =
  '{"v":1,"ts":"2026-09-24T02:10:00Z","seq":1,"event":"app_start","install_id":"a91d5c0e7b3f4a68b2c1d0e9f8a7b6c5","launch_id":"7f3c"}';

describe('PrivacyTab usage reporting section', () => {
  // Verifies: REQ-TEL-011
  it('usage reporting is not on, the switch is off and nothing is previewed', () => {
    render(<PrivacyTab onUsageReportingChange={() => {}} />);

    expect(screen.getByRole('switch', { name: 'Send usage data' })).not.toBeChecked();
    expect(screen.queryByTestId('privacy-usage-preview')).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  // Verifies: REQ-TEL-011
  it('the user turns the switch on, the handler gets true', () => {
    const onUsageReportingChange = vi.fn();
    render(<PrivacyTab onUsageReportingChange={onUsageReportingChange} />);

    fireEvent.click(screen.getByRole('switch', { name: 'Send usage data' }));

    expect(onUsageReportingChange).toHaveBeenCalledWith(true);
  });

  // Verifies: REQ-TEL-011
  it('usage reporting is on, the switch is on and turning it off gives false', () => {
    const onUsageReportingChange = vi.fn();
    render(
      <PrivacyTab usageReporting onUsageReportingChange={onUsageReportingChange} />
    );

    const toggle = screen.getByRole('switch', { name: 'Send usage data' });
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);

    expect(onUsageReportingChange).toHaveBeenCalledWith(false);
  });

  // Verifies: REQ-TEL-011
  it('no handler is given, the section is not shown', () => {
    render(<PrivacyTab />);

    expect(screen.queryByTestId('privacy-usage')).not.toBeInTheDocument();
  });

  // Verifies: REQ-TEL-012
  it('the section is shown, it says what is sent, what is not, and that device names can hold a name', () => {
    render(<PrivacyTab onUsageReportingChange={() => {}} />);

    const section = screen.getByTestId('privacy-usage');
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
      <PrivacyTab
       
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
      <PrivacyTab
       
        usageReporting
        onUsageReportingChange={() => {}}
        usagePreview={INSTALL_LINE}
      />
    );

    expect(screen.getByTestId('privacy-usage-preview')).toHaveTextContent(INSTALL_LINE);
  });

  // Verifies: REQ-TEL-013
  it('usage reporting is off and the preview is empty, it says nothing is sent and there is no ID', () => {
    render(
      <PrivacyTab onUsageReportingChange={() => {}} usagePreview="" />
    );

    expect(screen.getByTestId('privacy-usage-preview')).toHaveTextContent(
      'Off: nothing is collected or sent, and there is no install ID.'
    );
  });

  // Verifies: REQ-TEL-013
  it('usage reporting is on and nothing is waiting, it says so', () => {
    render(
      <PrivacyTab usageReporting onUsageReportingChange={() => {}} usagePreview="" />
    );

    expect(screen.getByTestId('privacy-usage-preview')).toHaveTextContent(
      'Nothing is waiting to be sent yet.'
    );
  });

  it('the preview could not be read, the reason is shown as an alert', () => {
    render(
      <PrivacyTab
       
        onUsageReportingChange={() => {}}
        usagePreviewError="usage_preview failed"
      />
    );

    expect(screen.getByRole('alert')).toHaveTextContent('usage_preview failed');
  });
});

describe('PrivacyTab previous hang section (ADR-056, ADR-059)', () => {
  const hang = { stage: 'restart' as const, stalled_ms: 9000 };

  // Verifies: REQ-TEL-020
  it('nothing is rendered when there is no pending hang', () => {
    render(<PrivacyTab onSendPreviousHang={() => {}} previousHang={null} />);

    expect(screen.queryByTestId('privacy-previous-hang')).not.toBeInTheDocument();
  });

  // Verifies: REQ-TEL-020
  it('nothing is rendered when there is no handler to answer it', () => {
    render(<PrivacyTab previousHang={hang} />);

    expect(screen.queryByTestId('privacy-previous-hang')).not.toBeInTheDocument();
  });

  // Verifies: REQ-TEL-020
  it('a pending hang with no attached log describes the report as not carrying audio, chat or room content', () => {
    render(
      <PrivacyTab
       
        onSendPreviousHang={() => {}}
        previousHang={hang}
        previousHangAttachesLog={false}
      />
    );

    expect(screen.getByTestId('privacy-previous-hang')).toHaveTextContent(
      'nothing about your audio, chat or rooms'
    );
  });

  // Verifies: REQ-TEL-021
  it('a pending hang that attaches the log says so and names what the log can carry', () => {
    render(
      <PrivacyTab
       
        onSendPreviousHang={() => {}}
        previousHang={hang}
        previousHangAttachesLog={true}
      />
    );

    const section = screen.getByTestId('privacy-previous-hang');
    expect(section).toHaveTextContent('jamuru.log');
    expect(section).toHaveTextContent('room ID');
    expect(section).not.toHaveTextContent('nothing about your audio, chat or rooms');
  });

  // Verifies: REQ-TEL-020
  it('pressing send answers with true', () => {
    const onSendPreviousHang = vi.fn();
    render(
      <PrivacyTab
       
        onSendPreviousHang={onSendPreviousHang}
        previousHang={hang}
      />
    );

    fireEvent.click(screen.getByTestId('privacy-previous-hang-send'));

    expect(onSendPreviousHang).toHaveBeenCalledWith(true);
  });

  // Verifies: REQ-TEL-020
  it('pressing dismiss answers with false', () => {
    const onSendPreviousHang = vi.fn();
    render(
      <PrivacyTab
       
        onSendPreviousHang={onSendPreviousHang}
        previousHang={hang}
      />
    );

    fireEvent.click(screen.getByTestId('privacy-previous-hang-dismiss'));

    expect(onSendPreviousHang).toHaveBeenCalledWith(false);
  });

  // Verifies: REQ-TEL-020
  it('once answered, a short confirmation replaces the report and its buttons', () => {
    render(
      <PrivacyTab
       
        onSendPreviousHang={() => {}}
        previousHang={hang}
        previousHangAnswered={true}
      />
    );

    expect(screen.queryByTestId('privacy-previous-hang-send')).not.toBeInTheDocument();
    expect(screen.getByTestId('privacy-previous-hang')).toHaveTextContent('Okay.');
  });
});
