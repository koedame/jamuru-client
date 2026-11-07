/**
 * ConnectionPanel: the way into the connection and device check before joining
 */

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';
import { ConnectionPanel } from './ConnectionPanel';

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, defaultValue?: string) => defaultValue ?? key,
  }),
}));

describe('ConnectionPanel connection check link', () => {
  // Verifies: REQ-GUI-037
  it('the person presses the link, the check is asked for', () => {
    const onCheckConnection = vi.fn();
    render(<ConnectionPanel onCheckConnection={onCheckConnection} />);

    fireEvent.click(screen.getByTestId('connection-panel-check-connection'));

    expect(onCheckConnection).toHaveBeenCalledTimes(1);
  });

  // Verifies: REQ-GUI-037
  it('no handler is given, the link is not shown', () => {
    render(<ConnectionPanel />);

    expect(
      screen.queryByTestId('connection-panel-check-connection'),
    ).not.toBeInTheDocument();
  });
});
