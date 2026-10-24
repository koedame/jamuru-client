import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';

import i18n from '../../i18n';
import { StartupUpdate } from './StartupUpdate';

describe('the window a start shows while it updates the app', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('ja');
  });

  // Verifies: REQ-UPD-025
  it('while it checks, it says so and the app can be opened without waiting', () => {
    const onSkip = vi.fn();
    render(<StartupUpdate progress={{ phase: 'checking' }} onSkip={onSkip} />);

    expect(screen.getByRole('status')).toHaveTextContent('アップデートを確認しています');
    fireEvent.click(screen.getByTestId('startup-update-skip'));
    expect(onSkip).toHaveBeenCalledTimes(1);
  });

  // Verifies: REQ-UPD-025
  it('while it downloads a download of known size, how far it is is shown in whole percent', () => {
    render(
      <StartupUpdate
        progress={{ phase: 'downloading', received: 2_999, total: 10_000 }}
        onSkip={() => {}}
      />
    );

    expect(screen.getByRole('status')).toHaveTextContent('新しい版をダウンロードしています29%');
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '29');
    expect(screen.getByTestId('startup-update-skip')).toBeEnabled();
  });

  // Verifies: REQ-UPD-025
  it('when how big the download is is not known, no percent is made up', () => {
    render(
      <StartupUpdate
        progress={{ phase: 'downloading', received: 2_999, total: null }}
        onSkip={() => {}}
      />
    );

    expect(screen.getByRole('status')).toHaveTextContent(/^新しい版をダウンロードしています$/);
    expect(screen.getByRole('progressbar')).not.toHaveAttribute('aria-valuenow');
  });

  // Verifies: REQ-UPD-025
  it('once it is installing or restarting, opening without waiting is no longer offered', () => {
    const { rerender } = render(<StartupUpdate progress={{ phase: 'installing' }} onSkip={() => {}} />);
    expect(screen.getByRole('status')).toHaveTextContent('新しい版を入れています');
    expect(screen.queryByTestId('startup-update-skip')).not.toBeInTheDocument();

    rerender(<StartupUpdate progress={{ phase: 'restarting' }} onSkip={() => {}} />);
    expect(screen.getByRole('status')).toHaveTextContent('再起動しています');
    expect(screen.queryByTestId('startup-update-skip')).not.toBeInTheDocument();
  });

  // Verifies: REQ-UPD-025
  it('once the person asked to open without waiting, the button cannot be pressed again', () => {
    render(<StartupUpdate progress={{ phase: 'checking' }} onSkip={() => {}} skipping />);

    expect(screen.getByTestId('startup-update-skip')).toBeDisabled();
  });

  // Verifies: REQ-UPD-025
  it('in English, it says the same in English', async () => {
    await i18n.changeLanguage('en');
    render(<StartupUpdate progress={{ phase: 'checking' }} onSkip={() => {}} />);

    expect(screen.getByRole('status')).toHaveTextContent('Checking for updates');
    expect(screen.getByTestId('startup-update-skip')).toHaveTextContent('Open without waiting');
  });
});
