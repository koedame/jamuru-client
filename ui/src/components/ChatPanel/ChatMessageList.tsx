/**
 * ChatMessageList - Scrollable message list that follows new messages while the
 * reader is at the bottom, with a date divider where the day changes.
 * MixerPanel design guide: minimal styling, functional focus
 */
import { Fragment, useEffect, useRef, type ReactNode } from "react";
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
  /** Follow new messages while the list is scrolled to (near) the bottom */
  autoScroll?: boolean;
  /** Locale for the date dividers */
  locale?: string;
}

/** How far from the bottom (px) the list still counts as "at the bottom". */
const FOLLOW_THRESHOLD_PX = 48;

function dayKey(timestamp: number): string {
  const d = new Date(timestamp);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

function formatDay(timestamp: number, locale: string): string {
  return new Date(timestamp).toLocaleDateString(locale, {
    year: "numeric",
    month: "long",
    day: "numeric",
    weekday: "short",
  });
}

export function ChatMessageList({
  messages,
  renderMessage,
  emptyMessage,
  autoScroll = true,
  locale,
}: ChatMessageListProps) {
  const { t, i18n } = useTranslation();
  const dayLocale = locale ?? i18n.language;
  const containerRef = useRef<HTMLDivElement>(null);
  // Whether the reader is at the bottom. The list is re-rendered on every
  // poll, so scrolling on each render would drag someone reading older
  // messages back down; only follow while they are already at the bottom.
  const followingRef = useRef(true);

  const last = messages[messages.length - 1];
  const lastId = last?.id;
  const lastIsOwn = last?.type === "own";

  // Follow only when a message was added (not on every re-render). A message
  // the reader just sent always brings them to the bottom.
  useEffect(() => {
    const el = containerRef.current;
    if (!autoScroll || !el || lastId === undefined) return;
    if (lastIsOwn) followingRef.current = true;
    if (followingRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [lastId, lastIsOwn, messages.length, autoScroll]);

  const handleScroll = () => {
    const el = containerRef.current;
    if (!el) return;
    followingRef.current =
      el.scrollHeight - el.scrollTop - el.clientHeight <= FOLLOW_THRESHOLD_PX;
  };

  if (messages.length === 0) {
    return (
      <div className="chat-message-list chat-message-list--empty">
        <div className="chat-message-list__empty-text">{emptyMessage ?? t("chat.emptyMessage")}</div>
      </div>
    );
  }

  return (
    <div className="chat-message-list" ref={containerRef} onScroll={handleScroll}>
      <div className="chat-message-list__messages">
        {messages.map((message, i) => {
          const day = dayKey(message.timestamp);
          const startsDay =
            i === 0 ? day !== dayKey(Date.now()) : day !== dayKey(messages[i - 1].timestamp);
          return (
            <Fragment key={message.id}>
              {startsDay && (
                <div
                  className="chat-message-list__date"
                  role="separator"
                  data-testid="chat-date-separator"
                >
                  {formatDay(message.timestamp, dayLocale)}
                </div>
              )}
              <div className="chat-message-list__item">{renderMessage(message)}</div>
            </Fragment>
          );
        })}
      </div>
    </div>
  );
}

export default ChatMessageList;
