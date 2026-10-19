import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { ChatMessage } from "./ChatMessage";

const meta = {
  title: "Components/Chat/ChatMessage",
  component: ChatMessage,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div
        style={{
          width: "300px",
          padding: "16px",
          background: "var(--color-bg-secondary)",
        }}
      >
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof ChatMessage>;

export default meta;
type Story = StoryObj<typeof meta>;

const now = Date.now();

export const OwnMessage: Story = {
  args: {
    type: "own",
    content: "これは自分のメッセージです",
    timestamp: now,
  },
};

export const OtherMessage: Story = {
  args: {
    type: "other",
    senderName: "Alice",
    content: "これは他の人のメッセージです",
    timestamp: now,
  },
};

export const MessageWithLink: Story = {
  args: {
    type: "other",
    senderName: "Alice",
    content: "資料はこちら https://example.com/docs/chat-links?page=1 です。\n(参考: https://example.org/a_(b))",
    timestamp: now,
    onOpenLink: fn(),
  },
};

export const SystemMessage: Story = {
  args: {
    type: "system",
    content: "Alice が参加しました",
    timestamp: now,
  },
};

export const LongOwnMessage: Story = {
  args: {
    type: "own",
    content:
      "これは長いメッセージのテストです。複数行にわたるテキストがどのように表示されるかを確認します。\n\n改行も含まれています。セッション中の詳細な説明や、曲の構成についての議論などに対応できるようにしています。",
    timestamp: now,
  },
};

export const LongOtherMessage: Story = {
  args: {
    type: "other",
    senderName: "Bob",
    content:
      "了解しました！次の曲はBPM120で、イントロ→Aメロ→Bメロ→サビの構成でいきましょう。\n\nドラムは8ビートで、ベースはルート中心にお願いします。",
    timestamp: now,
  },
};

export const ShortMessage: Story = {
  args: {
    type: "own",
    content: "OK",
    timestamp: now,
  },
};

export const JapaneseLocale: Story = {
  args: {
    type: "other",
    senderName: "田中",
    content: "日本語のメッセージです",
    timestamp: now,
    locale: "ja-JP",
  },
};

export const EnglishLocale: Story = {
  args: {
    type: "other",
    senderName: "John",
    content: "This is an English message",
    timestamp: now,
    locale: "en-US",
  },
};

export const WithReactions: Story = {
  args: {
    type: "other",
    senderName: "Alice",
    content: "次の曲はBPM120でいきましょう！",
    timestamp: now,
    reactions: [
      { emoji: "👍", count: 3, isActive: false },
      { emoji: "🎵", count: 2, isActive: true },
    ],
    onReactionClick: fn(),
  },
};

export const OwnMessageWithReactions: Story = {
  args: {
    type: "own",
    content: "了解！",
    timestamp: now,
    reactions: [
      { emoji: "👍", count: 2, isActive: true },
    ],
    onReactionClick: fn(),
  },
};

export const ManyReactions: Story = {
  args: {
    type: "other",
    senderName: "Bob",
    content: "今日のセッション最高だった！",
    timestamp: now,
    reactions: [
      { emoji: "👍", count: 5, isActive: true },
      { emoji: "❤️", count: 3, isActive: false },
      { emoji: "😄", count: 2, isActive: false },
      { emoji: "🎵", count: 4, isActive: true },
      { emoji: "👏", count: 6, isActive: false },
    ],
    onReactionClick: fn(),
  },
};

export const WithAddReaction: Story = {
  args: {
    type: "other",
    senderName: "Alice",
    content: "メッセージにホバーするとリアクション追加UIが表示されます",
    timestamp: now,
    onAddReaction: fn(),
  },
};

export const WithReactionsAndAddReaction: Story = {
  args: {
    type: "other",
    senderName: "Alice",
    content: "既存のリアクションがあり、さらに追加もできます",
    timestamp: now,
    reactions: [
      { emoji: "👍", count: 2, isActive: false },
      { emoji: "🎵", count: 1, isActive: true },
    ],
    onReactionClick: fn(),
    onAddReaction: fn(),
    recentEmojis: ["🔥", "✨", "🎉"],
  },
};
