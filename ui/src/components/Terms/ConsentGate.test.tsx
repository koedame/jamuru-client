import { Component, type ReactNode } from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, act } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';

import i18n from '../../i18n';
import { ConsentGate } from './ConsentGate';

const termsGet = vi.fn();
vi.mock('../../lib/tauri', () => ({
  termsGet: (...args: unknown[]) => termsGet(...args),
  termsAccept: vi.fn(),
}));

class Boundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? <p>failed</p> : this.props.children;
  }
}

function Broken(): ReactNode {
  throw new Error('cannot draw');
}

describe('the consent gate', () => {
  beforeEach(async () => {
    termsGet.mockReset();
    termsGet.mockResolvedValue({ version: 1, accepted: true, text: '# 利用規約' });
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-UPD-027
  it('what it shows is drawn, it says so once', async () => {
    const onShown = vi.fn();

    const { rerender } = render(<ConsentGate onShown={onShown}>main</ConsentGate>);
    expect(await screen.findByText('main')).toBeInTheDocument();
    rerender(<ConsentGate onShown={onShown}>main again</ConsentGate>);

    await waitFor(() => expect(onShown).toHaveBeenCalledTimes(1));
  });

  // A version whose screen fails to draw has not started (ADR-089).
  //
  // Verifies: REQ-UPD-027
  it('what it shows fails to draw, it does not say it is shown', async () => {
    const onShown = vi.fn();
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});

    render(
      <Boundary>
        <ConsentGate onShown={onShown}>
          <Broken />
        </ConsentGate>
      </Boundary>
    );

    expect(await screen.findByText('failed')).toBeInTheDocument();
    await act(async () => {
      await Promise.resolve();
    });
    expect(onShown).not.toHaveBeenCalled();
    error.mockRestore();
  });

  // Verifies: REQ-UPD-027
  it('the terms are still being read, it does not say it is shown yet', async () => {
    const onShown = vi.fn();
    termsGet.mockReturnValue(new Promise(() => {}));

    render(<ConsentGate onShown={onShown}>main</ConsentGate>);
    await act(async () => {
      await Promise.resolve();
    });

    expect(onShown).not.toHaveBeenCalled();
  });
});
