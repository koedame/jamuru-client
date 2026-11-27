/**
 * ConnectionIndicator component tests
 */

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';
import { ConnectionIndicator, type ConnectionStatus } from './ConnectionIndicator';

// Mock i18next
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (
      key: string,
      options?: {
        ms?: number;
        input?: number;
        output?: number;
        defaultValue?: string;
      }
    ) => {
      const translations: Record<string, string> = {
        'quality.good': 'Good',
        'quality.fair': 'Fair',
        'quality.poor': 'Poor',
        'status.disconnected': 'Disconnected',
        'status.connecting': 'Connecting...',
        'status.connected': 'Connected',
        'status.unstable': 'Unstable',
        'status.error': 'Disconnected',
      };
      if (key === 'status.latency' && options?.ms !== undefined) {
        return `${options.ms}ms`;
      }
      if (key === 'status.deviceLatency') {
        return `in ${options?.input}ms / out ${options?.output}ms`;
      }
      return translations[key] || options?.defaultValue || key;
    },
  }),
}));

describe('ConnectionIndicator', () => {
  const statuses: ConnectionStatus[] = [
    'disconnected',
    'connecting',
    'connected',
    'unstable',
    'error',
  ];

  describe('Status Display', () => {
    it.each(statuses)('displays correct text for %s status', (status: ConnectionStatus) => {
      render(<ConnectionIndicator status={status} />);
      const element = screen.getByRole('status');
      expect(element).toBeInTheDocument();
    });

    it('displays disconnected status correctly', () => {
      render(<ConnectionIndicator status="disconnected" />);
      expect(screen.getByText('Disconnected')).toBeInTheDocument();
    });

    it('displays connecting status correctly', () => {
      render(<ConnectionIndicator status="connecting" />);
      expect(screen.getByText('Connecting...')).toBeInTheDocument();
    });

    it('displays connected status correctly', () => {
      render(<ConnectionIndicator status="connected" />);
      expect(screen.getByText('Connected')).toBeInTheDocument();
    });

    it('displays unstable status correctly', () => {
      render(<ConnectionIndicator status="unstable" />);
      expect(screen.getByText('Unstable')).toBeInTheDocument();
    });

    it('displays error status correctly', () => {
      render(<ConnectionIndicator status="error" />);
      // Error status shows "Disconnected" text
      expect(screen.getByText('Disconnected')).toBeInTheDocument();
    });
  });

  describe('Latency Display', () => {
    it('displays latency when latencyMs is provided', () => {
      render(<ConnectionIndicator status="connected" latencyMs={15} />);
      expect(screen.getByText('(15ms)')).toBeInTheDocument();
    });

    it('does not display latency when latencyMs is undefined', () => {
      render(<ConnectionIndicator status="connected" />);
      expect(screen.queryByText(/ms\)/)).not.toBeInTheDocument();
    });

    it('hides latency when showLatency is false', () => {
      render(
        <ConnectionIndicator status="connected" latencyMs={15} showLatency={false} />
      );
      expect(screen.queryByText('(15ms)')).not.toBeInTheDocument();
    });

    it('shows latency when showLatency is true (default)', () => {
      render(<ConnectionIndicator status="connected" latencyMs={15} showLatency={true} />);
      expect(screen.getByText('(15ms)')).toBeInTheDocument();
    });
  });

  describe('Size Variants', () => {
    it('applies sm size class', () => {
      render(<ConnectionIndicator status="connected" size="sm" />);
      const element = screen.getByRole('status');
      expect(element.className).toContain('connection-indicator--sm');
    });

    it('applies md size class (default)', () => {
      render(<ConnectionIndicator status="connected" />);
      const element = screen.getByRole('status');
      expect(element.className).toContain('connection-indicator--md');
    });

    it('applies lg size class', () => {
      render(<ConnectionIndicator status="connected" size="lg" />);
      const element = screen.getByRole('status');
      expect(element.className).toContain('connection-indicator--lg');
    });
  });

  describe('Click Handler', () => {
    it('calls onClick when clicked', () => {
      const handleClick = vi.fn();
      render(<ConnectionIndicator status="connected" onClick={handleClick} />);

      const element = screen.getByRole('status');
      fireEvent.click(element);

      expect(handleClick).toHaveBeenCalledTimes(1);
    });

    it('does not have tabIndex when onClick is not provided', () => {
      render(<ConnectionIndicator status="connected" />);
      const element = screen.getByRole('status');
      expect(element).not.toHaveAttribute('tabIndex');
    });

    it('has tabIndex 0 when onClick is provided', () => {
      render(<ConnectionIndicator status="connected" onClick={() => {}} />);
      const element = screen.getByRole('status');
      expect(element).toHaveAttribute('tabIndex', '0');
    });

    it('triggers onClick on Enter key', () => {
      const handleClick = vi.fn();
      render(<ConnectionIndicator status="connected" onClick={handleClick} />);

      const element = screen.getByRole('status');
      fireEvent.keyDown(element, { key: 'Enter' });

      expect(handleClick).toHaveBeenCalledTimes(1);
    });

    it('triggers onClick on Space key', () => {
      const handleClick = vi.fn();
      render(<ConnectionIndicator status="connected" onClick={handleClick} />);

      const element = screen.getByRole('status');
      fireEvent.keyDown(element, { key: ' ' });

      expect(handleClick).toHaveBeenCalledTimes(1);
    });

    it('applies clickable class when onClick is provided', () => {
      render(<ConnectionIndicator status="connected" onClick={() => {}} />);
      const element = screen.getByRole('status');
      expect(element.className).toContain('connection-indicator--clickable');
    });
  });

  describe('Accessibility', () => {
    it('has role="status"', () => {
      render(<ConnectionIndicator status="connected" />);
      expect(screen.getByRole('status')).toBeInTheDocument();
    });

    it('has aria-live="polite"', () => {
      render(<ConnectionIndicator status="connected" />);
      const element = screen.getByRole('status');
      expect(element).toHaveAttribute('aria-live', 'polite');
    });

    it('has aria-label with status text', () => {
      render(<ConnectionIndicator status="connected" />);
      const element = screen.getByRole('status');
      expect(element).toHaveAttribute('aria-label', 'Connected');
    });

    it('has aria-label with status and latency', () => {
      render(<ConnectionIndicator status="connected" latencyMs={15} />);
      const element = screen.getByRole('status');
      expect(element).toHaveAttribute('aria-label', 'Connected (15ms)');
    });

    it('icon is hidden from screen readers', () => {
      render(<ConnectionIndicator status="connected" />);
      const element = screen.getByRole('status');
      const icon = element.querySelector('[aria-hidden="true"]');
      expect(icon).toBeInTheDocument();
    });
  });

  describe('CSS Classes', () => {
    it.each(statuses)('applies correct status class for %s', (status: ConnectionStatus) => {
      render(<ConnectionIndicator status={status} />);
      const element = screen.getByRole('status');
      expect(element.className).toContain(`connection-indicator--${status}`);
    });

    it('has base class', () => {
      render(<ConnectionIndicator status="connected" />);
      const element = screen.getByRole('status');
      expect(element.className).toContain('connection-indicator');
    });
  });

  describe('Quality band', () => {
    // The band is classified in the core library; the component only colours by
    // it. These check the mapping, not the thresholds.
    const bands = ['good', 'fair', 'poor'] as const;

    it.each(bands)('applies the %s quality class', (quality) => {
      const { container } = render(
        <ConnectionIndicator status="connected" quality={quality} />
      );
      expect(container.firstChild).toHaveClass(
        `connection-indicator--quality-${quality}`
      );
    });

    it('emits no quality class when there is no measurement yet', () => {
      const { container } = render(<ConnectionIndicator status="connecting" />);
      const className = (container.firstChild as HTMLElement).className;
      expect(className).not.toContain('connection-indicator--quality-');
    });

    it('names the band in the accessible label', () => {
      render(<ConnectionIndicator status="connected" quality="poor" />);
      expect(screen.getByRole('status')).toHaveAttribute(
        'aria-label',
        expect.stringContaining('Poor')
      );
    });

    it('keeps the status class so status and quality are both visible', () => {
      const { container } = render(
        <ConnectionIndicator status="unstable" quality="fair" />
      );
      expect(container.firstChild).toHaveClass('connection-indicator--unstable');
      expect(container.firstChild).toHaveClass(
        'connection-indicator--quality-fair'
      );
    });
  });

  describe('Device latency', () => {
    it('shows input and output latency separately from network latency', () => {
      render(
        <ConnectionIndicator
          status="connected"
          latencyMs={20}
          inputLatencyMs={3}
          outputLatencyMs={3}
        />
      );
      expect(screen.getByText('in 3ms / out 3ms')).toBeInTheDocument();
      // Network latency is still shown, and separately.
      expect(screen.getByText('(20ms)')).toBeInTheDocument();
    });

    it('shows nothing when only one direction is known', () => {
      render(<ConnectionIndicator status="connected" inputLatencyMs={3} />);
      expect(screen.queryByText(/in 3ms/)).not.toBeInTheDocument();
    });

    it('respects showLatency', () => {
      render(
        <ConnectionIndicator
          status="connected"
          inputLatencyMs={3}
          outputLatencyMs={5}
          showLatency={false}
        />
      );
      expect(screen.queryByText(/in 3ms/)).not.toBeInTheDocument();
    });
  });
});
