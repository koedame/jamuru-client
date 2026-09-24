/**
 * 他の参加者・サーバーから届く文字列の表示。
 *
 * 画面は React の既定のエスケープだけで文字列を出している。HTML を解釈する部品に
 * 変えると、相手が送った `<img onerror=...>` が端末で実行される。ここでは、
 * 攻撃用の文字列が要素にならず、文字としてそのまま出ることを確かめる。
 */

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';

import { ChatMessage } from './ChatPanel/ChatMessage';
import { ConnectionHistory } from './ConnectionHistory';

const PAYLOAD = '<img src=x onerror=alert(1)>';

describe('ChatMessage', () => {
  // Verifies: REQ-GUI-023
  it('本文が HTML を含むとき、要素にならず文字として表示されること', () => {
    const { container } = render(
      <ChatMessage type="other" senderName="Bob" content={PAYLOAD} timestamp={0} />
    );

    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('.chat-message__content')).toHaveTextContent(PAYLOAD);
  });

  // Verifies: REQ-GUI-023
  it('送り主の名前が HTML を含むとき、要素にならず文字として表示されること', () => {
    const { container } = render(
      <ChatMessage type="other" senderName={PAYLOAD} content="hello" timestamp={0} />
    );

    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('.chat-message__sender')).toHaveTextContent(PAYLOAD);
  });

  // Verifies: REQ-GUI-023
  it('入退室の通知に載る相手の名前が HTML を含むとき、要素にならず文字として表示されること', () => {
    for (const systemKind of ['join', 'leave'] as const) {
      const { container, unmount } = render(
        <ChatMessage type="system" systemKind={systemKind} senderName={PAYLOAD} content="" timestamp={0} />
      );

      expect(container.querySelector('img')).toBeNull();
      expect(container.querySelector('.chat-message__system-content')).toHaveTextContent(PAYLOAD);
      unmount();
    }
  });
});

describe('ConnectionHistory', () => {
  // Verifies: REQ-GUI-023
  it('ルームの表示名が HTML を含むとき、要素にならず文字として表示されること', () => {
    const { container } = render(
      <ConnectionHistory
        history={[{ room_code: 'ABC123', label: PAYLOAD, connected_at: '2026-09-24T00:00:00Z' }]}
        onSelect={() => {}}
        onRemove={() => {}}
      />
    );

    expect(container.querySelector('img')).toBeNull();
    expect(screen.getByText(PAYLOAD)).toHaveClass('connection-history__label');
  });
});
