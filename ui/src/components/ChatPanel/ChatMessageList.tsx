/**
 * ChatMessageList - Scrollable message list with auto-scroll
 * MixerPanel design guide: minimal styling, functional focus
 */
import { useEffect, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { Reaction } from "./ReactionBar";
import type { ChatSystemKind } from "./ChatMessage";
import "./ChatMessageList.css";

export interface ChatMessageData {
  id: string;
  type: "own" | "other" | "system";
  senderName?: string;
  content: string;
  timestamp: number;
  reactions?: Reaction[];
  /**
   * What a system message is about ("join"/"leave", or help with settings),
   * straight from the backend. Undefined for non-system messages and
   * for callers (e.g. Storybook mocks) that don't set it - ChatMessage falls
   * back to inferring it from `content` in that case.
   */
  systemKind?: ChatSystemKind | null;
  /** For a settings help line: who helped */
  helperName?: string | null;
  /** For "settings_help_changed": the setting that changed */
  setting?: string | null;
}

export interface ChatMessageListProps {
  /** Messages to display */
  messages: ChatMessageData[];
  /** Render function for each message */
  renderMessage: (message: ChatMessageData) => ReactNode;
  /** Empty state message */
  emptyMessage?: string;
  /** Auto-scroll to bottom on new messages */
  autoScroll?: boolean;
}

export function ChatMessageList({
  messages,
  renderMessage,
  emptyMessage,
  autoScroll = true,
}: ChatMessageListProps) {
  const { t } = useTranslation();
  const containerRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to bottom when messages change
  useEffect(() => {
    if (autoScroll && bottomRef.current) {
      bottomRef.current.scrollIntoView({ behavior: "smooth" });
    }
  }, [messages, autoScroll]);

  if (messages.length === 0) {
    return (
      <div className="chat-message-list chat-message-list--empty">
        <div className="chat-message-list__empty-text">{emptyMessage ?? t("chat.emptyMessage")}</div>
      </div>
    );
  }

  return (
    <div className="chat-message-list" ref={containerRef}>
      <div className="chat-message-list__messages">
        {messages.map((message) => (
          <div key={message.id} className="chat-message-list__item">
            {renderMessage(message)}
          </div>
        ))}
        <div ref={bottomRef} />
      </div>
    </div>
  );
}

export default ChatMessageList;
