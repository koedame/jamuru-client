/**
 * ChatMessage - Single chat message display
 * jamjam brand guide (ui.pen Screens/Main chatPanel): flat layout with a
 * sender/timestamp header row above the message body, and system (join/leave)
 * messages shown inline with a lucide log-in/log-out icon.
 */
import { useTranslation } from "react-i18next";
import { ReactionBar, type Reaction } from "./ReactionBar";
import { AddReactionButton } from "./AddReactionButton";
import { settingLabel } from "../SettingsHelp/settingText";
import "./ChatMessage.css";

export type ChatMessageType = "own" | "other" | "system";

/** What a system line is about: someone joining or leaving, or help with settings (ADR-043) */
export type ChatSystemKind =
  | "join"
  | "leave"
  | "settings_help_started"
  | "settings_help_changed"
  | "settings_help_ended";

export interface ChatMessageProps {
  /** Message type determines visual style */
  type: ChatMessageType;
  /**
   * Sender name. For a system message carrying `systemKind` it is the
   * participant the event is about (empty when unknown).
   */
  senderName?: string;
  /** Message content (ignored for a system message carrying `systemKind`) */
  content: string;
  /** Timestamp in milliseconds */
  timestamp: number;
  /** Optional: format time in specific locale (defaults to the UI language) */
  locale?: string;
  /** Optional: reactions on this message */
  reactions?: Reaction[];
  /** Optional: callback when a reaction is clicked */
  onReactionClick?: (emoji: string) => void;
  /** Optional: callback when adding a new reaction */
  onAddReaction?: (emoji: string) => void;
  /** Recently used emojis for picker */
  recentEmojis?: string[];
  /** Show action bar on hover (default: true if onAddReaction is provided) */
  showActions?: boolean;
  /**
   * "join"/"leave" for a system message, straight from the backend. The
   * text is then rendered from the UI language and `senderName`, so the
   * backend sends no display text. When omitted (e.g. Storybook mocks),
   * `content` is shown as-is and the kind is inferred from it - see
   * `inferSystemKind`.
   */
  systemKind?: ChatSystemKind | null;
  /** For a settings help line: who helped (`senderName` is who was helped) */
  helperName?: string | null;
  /** For "settings_help_changed": the setting that changed, as a settings change names it */
  setting?: string | null;
}

/**
 * Format timestamp to HH:MM format
 */
function formatTime(timestamp: number, locale: string): string {
  const date = new Date(timestamp);
  return date.toLocaleTimeString(locale, {
    hour: "2-digit",
    minute: "2-digit",
  });
}

/**
 * Fallback classifier used only when the caller doesn't supply `systemKind`
 * (e.g. Storybook mocks). Keyword matching on free text is inherently
 * unreliable - e.g. a display name containing "leave" as a substring - so
 * real messages should always come with the backend's authoritative field.
 */
export function inferSystemKind(content: string): "join" | "leave" | null {
  if (/参加|入室|join/i.test(content)) return "join";
  if (/退出|退室|left|leave/i.test(content)) return "leave";
  return null;
}

function LogInIcon() {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4" />
      <polyline points="10 17 15 12 10 7" />
      <line x1="15" y1="12" x2="3" y2="12" />
    </svg>
  );
}

function SlidersIcon() {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <line x1="4" y1="21" x2="4" y2="14" />
      <line x1="4" y1="10" x2="4" y2="3" />
      <line x1="12" y1="21" x2="12" y2="12" />
      <line x1="12" y1="8" x2="12" y2="3" />
      <line x1="20" y1="21" x2="20" y2="16" />
      <line x1="20" y1="12" x2="20" y2="3" />
      <line x1="1" y1="14" x2="7" y2="14" />
      <line x1="9" y1="8" x2="15" y2="8" />
      <line x1="17" y1="16" x2="23" y2="16" />
    </svg>
  );
}

function LogOutIcon() {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
      <polyline points="16 17 21 12 16 7" />
      <line x1="21" y1="12" x2="9" y2="12" />
    </svg>
  );
}

export function ChatMessage({
  type,
  senderName,
  content,
  timestamp,
  locale,
  reactions = [],
  onReactionClick,
  onAddReaction,
  recentEmojis = [],
  showActions,
  systemKind,
  helperName,
  setting,
}: ChatMessageProps) {
  const { t, i18n } = useTranslation();
  const timeLocale = locale ?? i18n.language;
  const shouldShowActions = showActions ?? !!onAddReaction;

  if (type === "system") {
    const kind = systemKind ?? inferSystemKind(content);
    let text = content;
    const names = { helper: helperName ?? "", helped: senderName ?? "" };
    if (systemKind === "join") {
      text = t("chat.system.joined", { name: senderName });
    } else if (systemKind === "leave") {
      text = senderName
        ? t("chat.system.left", { name: senderName })
        : t("chat.system.leftUnknown");
    } else if (systemKind === "settings_help_started") {
      text = t("chat.system.settingsHelpStarted", names);
    } else if (systemKind === "settings_help_changed") {
      text = t("chat.system.settingsHelpChanged", { ...names, setting: settingLabel(setting, t) });
    } else if (systemKind === "settings_help_ended") {
      text = t("chat.system.settingsHelpEnded", names);
    }
    return (
      <div className="chat-message chat-message--system" data-system-kind={kind ?? undefined}>
        {kind && (
          <span className="chat-message__system-icon">
            {kind === "join" ? <LogInIcon /> : kind === "leave" ? <LogOutIcon /> : <SlidersIcon />}
          </span>
        )}
        <span className="chat-message__system-content">{text}</span>
        <span className="chat-message__time">{formatTime(timestamp, timeLocale)}</span>
      </div>
    );
  }

  return (
    <div className={`chat-message chat-message--${type}`} data-testid="chat-message" data-message-type={type}>
      <div className="chat-message__header">
        {senderName && <span className="chat-message__sender">{senderName}</span>}
        <span className="chat-message__time">{formatTime(timestamp, timeLocale)}</span>
      </div>
      <div className="chat-message__content">{content}</div>
      {(reactions.length > 0 || shouldShowActions) && (
        <div className="chat-message__reactions">
          {reactions.length > 0 && (
            <ReactionBar reactions={reactions} onReactionClick={onReactionClick} />
          )}
          {shouldShowActions && (
            <AddReactionButton
              onSelect={onAddReaction}
              recentEmojis={recentEmojis}
            />
          )}
        </div>
      )}
    </div>
  );
}

export default ChatMessage;
