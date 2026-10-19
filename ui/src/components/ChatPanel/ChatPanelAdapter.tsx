/**
 * ChatPanelAdapter - Docks the Props-based ChatPanel as the connected screen's
 * right-hand column and wires it to the Tauri backend for real-time chat.
 */
import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "../../lib/invoke";
import { ChatPanel } from "./ChatPanel";
import type { ChatMessageData } from "./ChatMessageList";
import type { Reaction as ChatPanelReaction } from "./ReactionBar";
import {
  signalingToggleReaction,
  signalingAddReaction,
  chatOpenLink,
  type ChatMessage as TauriChatMessage,
  type Reaction as TauriReaction,
} from "../../lib/tauri";

// LocalStorage key for recent emojis
const RECENT_EMOJIS_KEY = "chatPanel:recentEmojis";
const MAX_RECENT_EMOJIS = 8;

export interface ChatPanelAdapterProps {
  /** Connection ID for Tauri API */
  connId: number | null;
  /** Current user's peer ID */
  myPeerId: string | null;
}

/**
 * Convert Tauri reaction format to ChatPanel format
 */
function convertReaction(reaction: TauriReaction, myPeerId: string | null): ChatPanelReaction {
  return {
    emoji: reaction.emoji,
    count: reaction.count,
    isActive: myPeerId ? reaction.user_ids.includes(myPeerId) : false,
  };
}

/**
 * Convert Tauri message format to ChatPanel format
 */
function convertMessage(msg: TauriChatMessage, myPeerId: string | null): ChatMessageData {
  let type: "own" | "other" | "system";
  if (msg.is_system) {
    type = "system";
  } else if (myPeerId && msg.sender_id === myPeerId) {
    type = "own";
  } else {
    type = "other";
  }

  return {
    id: msg.id,
    type,
    senderName: msg.sender_name,
    content: msg.content,
    timestamp: msg.timestamp,
    reactions: msg.reactions?.map((r) => convertReaction(r, myPeerId)),
    systemKind: msg.system_kind,
    helperName: msg.helper_name,
    setting: msg.setting,
  };
}

/**
 * Load recent emojis from localStorage
 */
function loadRecentEmojis(): string[] {
  try {
    const stored = localStorage.getItem(RECENT_EMOJIS_KEY);
    if (stored) {
      return JSON.parse(stored);
    }
  } catch {
    // Ignore parse errors
  }
  return ["👍", "❤️", "😂", "😮", "😢", "🎵"];
}

/**
 * Save recent emojis to localStorage
 */
function saveRecentEmojis(emojis: string[]): void {
  try {
    localStorage.setItem(RECENT_EMOJIS_KEY, JSON.stringify(emojis));
  } catch {
    // Ignore storage errors
  }
}

/**
 * Add emoji to recent list (moves to front if already present)
 */
function addToRecentEmojis(emoji: string, current: string[]): string[] {
  const filtered = current.filter((e) => e !== emoji);
  const updated = [emoji, ...filtered].slice(0, MAX_RECENT_EMOJIS);
  saveRecentEmojis(updated);
  return updated;
}

export function ChatPanelAdapter({
  connId,
  myPeerId,
}: ChatPanelAdapterProps) {
  const [messages, setMessages] = useState<ChatMessageData[]>([]);
  const [recentEmojis, setRecentEmojis] = useState<string[]>(loadRecentEmojis);
  // The last snapshot from the backend, as text. A poll that brings the same
  // snapshot leaves the state alone so the list is not rebuilt every 500ms.
  const lastSnapshotRef = useRef<string | null>(null);

  const fetchMessages = useCallback(async () => {
    const fetched = await invoke<TauriChatMessage[]>("signaling_get_chat_messages", {
      sinceTimestamp: null,
    });
    const snapshot = JSON.stringify(fetched);
    if (snapshot === lastSnapshotRef.current) return;
    lastSnapshotRef.current = snapshot;
    setMessages(fetched.map((msg) => convertMessage(msg, myPeerId)));
  }, [myPeerId]);

  // Poll for new messages while connected (the chat is a permanent column).
  useEffect(() => {
    if (connId === null) return;

    lastSnapshotRef.current = null;
    const pollMessages = async () => {
      try {
        await fetchMessages();
      } catch {
        // Silently ignore errors during polling
      }
    };

    // Initial fetch
    pollMessages();

    // Poll every 500ms
    const interval = setInterval(pollMessages, 500);
    return () => clearInterval(interval);
  }, [connId, fetchMessages]);

  // Rejects when the message could not be sent, so the input keeps the text
  // and tells the reader.
  const handleSend = useCallback(
    async (content: string) => {
      if (!content.trim() || connId === null) return;

      try {
        await invoke("signaling_send_chat", {
          connId,
          content: content.trim(),
        });
      } catch (err) {
        console.error("Failed to send chat message:", err);
        throw err;
      }
      // The backend already holds the sent message; show it now instead of
      // at the next poll.
      try {
        await fetchMessages();
      } catch {
        // The next poll shows it
      }
    },
    [connId, fetchMessages]
  );

  // Handle clicking on an existing reaction (toggle)
  const handleReactionClick = useCallback(
    async (messageId: string, emoji: string) => {
      try {
        await signalingToggleReaction(messageId, emoji);
        // Update recent emojis
        setRecentEmojis((prev) => addToRecentEmojis(emoji, prev));
      } catch (err) {
        console.error("Failed to toggle reaction:", err);
      }
    },
    []
  );

  // Handle adding a new reaction from the picker
  const handleAddReaction = useCallback(
    async (messageId: string, emoji: string) => {
      try {
        await signalingAddReaction(messageId, emoji);
        // Update recent emojis
        setRecentEmojis((prev) => addToRecentEmojis(emoji, prev));
      } catch (err) {
        console.error("Failed to add reaction:", err);
      }
    },
    []
  );

  const handleOpenLink = useCallback(async (url: string) => {
    try {
      await chatOpenLink(url);
    } catch (err) {
      console.error("Failed to open link:", err);
    }
  }, []);

  return (
    <ChatPanel
      messages={messages}
      onOpenLink={handleOpenLink}
      onSend={handleSend}
      onReactionClick={handleReactionClick}
      onAddReaction={handleAddReaction}
      recentEmojis={recentEmojis}
      disabled={connId === null}
    />
  );
}

export default ChatPanelAdapter;
