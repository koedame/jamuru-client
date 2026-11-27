/**
 * Renders the panels of the connected screen (mixer, room sidebar meter, chat,
 * leave dialog) in each UI language and checks that only that language shows.
 *
 * The bug this guards: with the language set to English, the mixer title and
 * the chat title / placeholder stayed Japanese while everything else was
 * English, because those strings bypassed the locale bundles.
 */

import { describe, it, expect, beforeAll, beforeEach, afterAll, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

import i18n from './index';
import en from '../../locales/en.json';
import ja from '../../locales/ja.json';
import { MixerPanel, MasterSection, type Channel } from '../components/MixerPanel';
import { ChatPanel, ChatPanelAdapter } from '../components/ChatPanel';
import { LeaveDialog } from '../components/LeaveDialog';
import { ConnectionPanel } from '../components/ConnectionPanel';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const JAPANESE = /[぀-ヿ㐀-鿿]/;

const channels: Channel[] = [
  {
    id: 'local',
    name: 'Me',
    type: 'local',
    sampleRate: 48000,
    channelCount: 1,
    levelL: 10,
    levelR: 10,
    volume: 80,
    pan: 0,
    isMuted: false,
    isMonitoring: false,
  },
  {
    id: 'peer-1',
    name: 'Alice',
    type: 'remote',
    sampleRate: 48000,
    channelCount: 2,
    levelL: 10,
    levelR: 10,
    volume: 80,
    pan: 0,
    isMuted: true,
  },
];

/** Every string a user or a screen reader gets from the rendered DOM. */
function shownStrings(root: HTMLElement): string[] {
  const strings = [root.textContent ?? ''];
  for (const el of root.querySelectorAll('*')) {
    for (const attr of ['aria-label', 'title', 'placeholder']) {
      const value = el.getAttribute(attr);
      if (value) strings.push(value);
    }
  }
  return strings;
}

function renderConnectedPanels() {
  return render(
    <>
      <MixerPanel channels={channels} onChannelMonitorToggle={() => {}} />
      <MasterSection levelL={10} levelR={10} />
      <ChatPanel
        messages={[
          { id: '1', type: 'system', senderName: 'Alice', content: '', timestamp: 0, systemKind: 'join' },
          { id: '2', type: 'system', content: '', timestamp: 0, systemKind: 'leave' },
        ]}
        onSend={() => {}}
        onAddReaction={() => {}}
      />
      <LeaveDialog open onConfirm={() => {}} onCancel={() => {}} />
    </>
  );
}

const backendMessageCases = [
  ['en', 'Bob joined', 'A participant left'],
  ['ja', 'Bobさんが参加しました', '参加者が退出しました'],
] as const;

const connectionPanelCases = [
  ['en', en.session.welcome.title],
  ['ja', ja.session.welcome.title],
] as const;

describe('connected screen language', () => {
  beforeAll(() => {
    // jsdom has no layout, so the chat list's scroll-to-bottom is not implemented.
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(async () => {
    invoke.mockReset();
  });

  afterAll(async () => {
    await i18n.changeLanguage('en');
  });

  // Given the UI language is English
  // When the connected screen's panels are rendered without explicit texts
  // Then no Japanese appears anywhere in them
  it('shows no Japanese in English', async () => {
    await i18n.changeLanguage('en');
    const { container } = renderConnectedPanels();

    expect(shownStrings(container).filter((s) => JAPANESE.test(s))).toEqual([]);
    expect(screen.getByText(en.mixer.title)).toBeInTheDocument();
    expect(screen.getByText(en.chat.title)).toBeInTheDocument();
    expect(screen.getByPlaceholderText(en.chat.placeholder)).toBeInTheDocument();
  });

  // Given the UI language is Japanese
  // When the same panels are rendered
  // Then the titles, placeholder and system messages are the Japanese bundle's
  it('shows the Japanese bundle in Japanese', async () => {
    await i18n.changeLanguage('ja');
    renderConnectedPanels();

    expect(screen.getByText(ja.mixer.title)).toBeInTheDocument();
    expect(screen.getByText(ja.mixer.master)).toBeInTheDocument();
    expect(screen.getByText(ja.chat.title)).toBeInTheDocument();
    expect(screen.getByPlaceholderText(ja.chat.placeholder)).toBeInTheDocument();
    expect(screen.getByText('Aliceさんが参加しました')).toBeInTheDocument();
    expect(screen.getByText(ja.chat.system.leftUnknown)).toBeInTheDocument();
    expect(screen.getByText(ja.session.leave.confirmMessage)).toBeInTheDocument();
  });

  // Given a system message carries only its kind and the participant's name
  // When the language changes while it is on screen
  // Then the message is re-rendered in the new language
  it('re-renders a system message when the language changes', async () => {
    await i18n.changeLanguage('en');
    const { rerender } = render(
      <ChatPanel
        messages={[{ id: '1', type: 'system', senderName: 'Alice', content: '', timestamp: 0, systemKind: 'join' }]}
      />
    );
    expect(screen.getByText('Alice joined')).toBeInTheDocument();

    await i18n.changeLanguage('ja');
    rerender(
      <ChatPanel
        messages={[{ id: '1', type: 'system', senderName: 'Alice', content: '', timestamp: 0, systemKind: 'join' }]}
      />
    );
    expect(screen.getByText('Aliceさんが参加しました')).toBeInTheDocument();
  });

  // Given the backend reports a join and a leave as kind + name, with no text
  // When the chat adapter shows them
  // Then they read in the current UI language
  it.each(backendMessageCases)('renders the backend join/leave messages in %s', async (language, joined, left) => {
    await i18n.changeLanguage(language);
    invoke.mockResolvedValue([
      { id: '1', sender_id: '', sender_name: 'Bob', content: '', timestamp: 0, is_system: true, system_kind: 'join', reactions: [] },
      { id: '2', sender_id: '', sender_name: '', content: '', timestamp: 0, is_system: true, system_kind: 'leave', reactions: [] },
    ]);

    render(<ChatPanelAdapter connId={1} myPeerId="me" />);

    expect(await screen.findByText(joined)).toBeInTheDocument();
    expect(screen.getByText(left)).toBeInTheDocument();
  });

  // Given the connection panel is rendered without any text props
  // When the language is English
  // Then no Japanese shows, and in Japanese the Japanese bundle does
  it.each(connectionPanelCases)('renders the connection panel defaults in %s', async (language, welcomeTitle) => {
    await i18n.changeLanguage(language);
    const { container } = render(<ConnectionPanel onCreateRoom={() => {}} onJoinRoom={() => {}} />);

    expect(screen.getByText(welcomeTitle)).toBeInTheDocument();
    if (language === 'en') {
      expect(shownStrings(container).filter((s) => JAPANESE.test(s))).toEqual([]);
    }
  });
});
