import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';

import i18n from '../../i18n';
import { EntryNameDialog } from './EntryNameDialog';

function open(props: Partial<React.ComponentProps<typeof EntryNameDialog>> = {}) {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  render(
    <EntryNameDialog
      open
      initialName="Aki"
      confirmLabel="Join"
      onConfirm={onConfirm}
      onCancel={onCancel}
      {...props}
    />
  );
  return { onConfirm, onCancel };
}

describe('the entry name dialog', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-CON-141
  it('it is open, it asks for the name with the given name in the field', () => {
    open();

    expect(screen.getByRole('dialog', { name: 'Enter your name' })).toBeInTheDocument();
    expect(screen.getByLabelText('Display name')).toHaveValue('Aki');
  });

  // Verifies: REQ-CON-141
  it('it is closed, it shows nothing', () => {
    open({ open: false });

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  // Verifies: REQ-CON-141
  it('the name is changed and confirmed, the new name is handed on without stopping the check', () => {
    const { onConfirm } = open();

    fireEvent.change(screen.getByLabelText('Display name'), { target: { value: '  Taro ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Join' }));

    expect(onConfirm).toHaveBeenCalledWith('Taro', false);
  });

  // Verifies: REQ-CON-142
  it('"use this name from now on" is ticked, confirming says to stop asking', () => {
    const { onConfirm } = open();

    fireEvent.click(screen.getByLabelText('Use this name from now on'));
    fireEvent.click(screen.getByRole('button', { name: 'Join' }));

    expect(onConfirm).toHaveBeenCalledWith('Aki', true);
  });

  // Verifies: REQ-CON-141
  it('the name is empty, it cannot be confirmed', () => {
    const { onConfirm } = open({ initialName: '' });

    expect(screen.getByRole('button', { name: 'Join' })).toBeDisabled();
    fireEvent.change(screen.getByLabelText('Display name'), { target: { value: '   ' } });
    expect(screen.getByRole('button', { name: 'Join' })).toBeDisabled();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  // Verifies: REQ-CON-141
  it('Enter is pressed in the field, the name is confirmed', () => {
    const { onConfirm } = open();

    fireEvent.submit(screen.getByLabelText('Display name'));

    expect(onConfirm).toHaveBeenCalledWith('Aki', false);
  });

  // Verifies: REQ-CON-141
  it('cancel or Escape is chosen, the dialog is cancelled and nothing is confirmed', () => {
    const { onConfirm, onCancel } = open();

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });

    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  // Verifies: REQ-CON-141
  it('the name is being saved, nothing can be pressed or cancelled', () => {
    const { onCancel } = open({ pending: true });

    expect(screen.getByRole('button', { name: 'Join' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(onCancel).not.toHaveBeenCalled();
  });

  // Verifies: REQ-CON-141
  it('the name could not be saved, the reason is shown', () => {
    open({ error: 'Could not save the name' });

    expect(screen.getByRole('alert')).toHaveTextContent('Could not save the name');
  });

  // Verifies: REQ-CON-141
  it('it is opened again, the field and the tick start over', () => {
    const onConfirm = vi.fn();
    const props = { initialName: 'Aki', confirmLabel: 'Join', onConfirm, onCancel: vi.fn() };
    const { rerender } = render(<EntryNameDialog open {...props} />);
    fireEvent.change(screen.getByLabelText('Display name'), { target: { value: 'Other' } });
    fireEvent.click(screen.getByLabelText('Use this name from now on'));

    rerender(<EntryNameDialog open={false} {...props} />);
    rerender(<EntryNameDialog open {...props} />);

    expect(screen.getByLabelText('Display name')).toHaveValue('Aki');
    expect(screen.getByLabelText('Use this name from now on')).not.toBeChecked();
  });
});
