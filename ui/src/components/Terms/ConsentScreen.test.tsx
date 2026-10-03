import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';

import i18n from '../../i18n';
import { ConsentScreen } from './ConsentScreen';

const TEXT = '# 利用規約\n\n## 第 1 条（本サービスでできること）\n\n1. 音声をやり取りします。';

describe('the consent screen', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en');
  });

  // Verifies: REQ-TRM-001
  it('it is shown, the terms and the line saying other people can see the IP address are on screen', () => {
    render(<ConsentScreen text={TEXT} onAccept={() => {}} />);

    expect(screen.getByText('第 1 条（本サービスでできること）')).toBeInTheDocument();
    expect(screen.getByTestId('consent-ip-notice')).toHaveTextContent(
      'the other people in the room can see your IP address'
    );
  });

  // Verifies: REQ-TRM-001
  it('neither box is checked, the button cannot be pressed', () => {
    render(<ConsentScreen text={TEXT} onAccept={() => {}} />);

    expect(screen.getByTestId('consent-start')).toBeDisabled();
  });

  // Verifies: REQ-TRM-001
  it('only one of the two boxes is checked, the button cannot be pressed', () => {
    const onAccept = vi.fn();
    render(<ConsentScreen text={TEXT} onAccept={onAccept} />);

    fireEvent.click(screen.getByTestId('consent-agree'));
    expect(screen.getByTestId('consent-start')).toBeDisabled();
    fireEvent.click(screen.getByTestId('consent-agree'));
    fireEvent.click(screen.getByTestId('consent-adult'));
    expect(screen.getByTestId('consent-start')).toBeDisabled();
    fireEvent.click(screen.getByTestId('consent-start'));

    expect(onAccept).not.toHaveBeenCalled();
  });

  // Verifies: REQ-TRM-001
  it('both boxes are checked, the button agrees once', () => {
    const onAccept = vi.fn();
    render(<ConsentScreen text={TEXT} onAccept={onAccept} />);

    fireEvent.click(screen.getByTestId('consent-agree'));
    fireEvent.click(screen.getByTestId('consent-adult'));
    fireEvent.click(screen.getByTestId('consent-start'));

    expect(onAccept).toHaveBeenCalledTimes(1);
  });

  it('the agreement is being saved, the button cannot be pressed again', () => {
    render(<ConsentScreen text={TEXT} onAccept={() => {}} accepting />);
    fireEvent.click(screen.getByTestId('consent-agree'));
    fireEvent.click(screen.getByTestId('consent-adult'));

    expect(screen.getByTestId('consent-start')).toBeDisabled();
  });

  it('the screen is shown in Japanese, the notice and the labels are in Japanese', async () => {
    await i18n.changeLanguage('ja');
    render(<ConsentScreen text={TEXT} onAccept={() => {}} />);

    expect(screen.getByTestId('consent-ip-notice')).toHaveTextContent('IP アドレスが見えます');
    expect(screen.getByText('同意して始める')).toBeInTheDocument();
  });
});
