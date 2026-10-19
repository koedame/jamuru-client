/**
 * ChatPanel - Complete chat interface for Storybook
 * MixerPanel design guide compliant, no Tauri dependencies
 */
import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { ChatMessage } from "./ChatMessage";
import { ChatMessageList, type ChatMessageData } from "./ChatMessageList";
import { ChatInput } from "./ChatInput";
import "./ChatPanel.css";

export interface ChatPanelProps {
  /** Chat messages to display */
  messages: ChatMessageData[];
  /**
   * Callback when a message is sent. If it returns a promise, the input keeps
   * the text until the promise resolves and shows an error when it rejects.
   */
  onSend?: (message: string) => void | Promise<void>;
  /** Callback when a reaction is clicked on a message */
  onReactionClick?: (messageId: string, emoji: string) => void;
  /** Callback when adding a new reaction to a message */
  onAddReaction?: (messageId: string, emoji: string) => void;
  /** Recently used emojis for picker */
  recentEmojis?: string[];
  /** Disable input */
  disabled?: boolean;
  /** Panel title */
  title?: string;
  /** Input placeholder */
  placeholder?: string;
  /** Empty state message */
  emptyMessage?: string;
  /** Locale for time formatting */
  locale?: string;
  /** Callback when a link in a message is clicked */
  onOpenLink?: (url: string) => void;
}

export function ChatPanel({
  messages,
  onSend,
  onReactionClick,
  onAddReaction,
  recentEmojis = [],
  disabled = false,
  title,
  placeholder,
  emptyMessage,
  locale,
  onOpenLink,
}: ChatPanelProps) {
  const { t, i18n } = useTranslation();
  const titleText = title ?? t("chat.title");
  const timeLocale = locale ?? i18n.language;
  const handleSend = useCallback(
    (content: string) => onSend?.(content),
    [onSend]
  );

  const renderMessage = useCallback(
    (message: ChatMessageData) => (
      <ChatMessage
        type={message.type}
        senderName={message.senderName}
        content={message.content}
        timestamp={message.timestamp}
        locale={timeLocale}
        reactions={message.reactions}
        systemKind={message.systemKind}
        helperName={message.helperName}
        setting={message.setting}
        onOpenLink={onOpenLink}
        onReactionClick={
          onReactionClick
            ? (emoji) => onReactionClick(message.id, emoji)
            : undefined
        }
        onAddReaction={
          onAddReaction
            ? (emoji) => onAddReaction(message.id, emoji)
            : undefined
        }
        recentEmojis={recentEmojis}
      />
    ),
    [timeLocale, onReactionClick, onAddReaction, recentEmojis, onOpenLink]
  );

  return (
    <div className="chat-panel-container" data-testid="chat-panel">
      {titleText && (
        <div className="chat-panel-container__header">
          <span className="chat-panel-container__title">{titleText}</span>
        </div>
      )}
      <ChatMessageList
        messages={messages}
        renderMessage={renderMessage}
        emptyMessage={emptyMessage}
        locale={timeLocale}
      />
      <ChatInput
        placeholder={placeholder}
        onSend={handleSend}
        disabled={disabled || !onSend}
      />
    </div>
  );
}

export default ChatPanel;
