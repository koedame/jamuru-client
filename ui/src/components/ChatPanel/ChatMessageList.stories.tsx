import type { Meta, StoryObj } from "@storybook/react-vite";
import { ChatMessageList, type ChatMessageData } from "./ChatMessageList";
import { ChatMessage } from "./ChatMessage";

const meta = {
  title: "Components/Chat/ChatMessageList",
  component: ChatMessageList,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div
        style={{
          width: "300px",
          height: "400px",
          padding: "16px",
          background: "var(--color-bg-secondary)",
        }}
      >
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof ChatMessageList>;

export default meta;
type Story = StoryObj<typeof meta>;

const now = Date.now();

const sampleMessages: ChatMessageData[] = [
  {
    id: "1",
    type: "system",
    content: "セッションが開始されました",
    timestamp: now - 300000,
  },
  {
    id: "2",
    type: "system",
    content: "Alice が参加しました",
    timestamp: now - 290000,
  },
  {
    id: "3",
    type: "other",
    senderName: "Alice",
    content: "こんにちは！今日もよろしくお願いします。",
    timestamp: now - 280000,
  },
  {
    id: "4",
    type: "own",
    content: "よろしく！何から始める？",
    timestamp: now - 270000,
  },
  {
    id: "5",
    type: "other",
    senderName: "Alice",
    content: "BPM120のファンクでいこう",
    timestamp: now - 260000,
  },
  {
    id: "6",
    type: "own",
    content: "OK、いいね！",
    timestamp: now - 250000,
  },
];

const renderMessage = (message: ChatMessageData) => (
  <ChatMessage
    type={message.type}
    senderName={message.senderName}
    content={message.content}
    timestamp={message.timestamp}
  />
);

export const Default: Story = {
  args: {
    messages: sampleMessages,
    renderMessage,
  },
};

export const Empty: Story = {
  args: {
    messages: [],
    renderMessage,
    emptyMessage: "メッセージはまだありません",
  },
};

export const CustomEmptyMessage: Story = {
  args: {
    messages: [],
    renderMessage,
    emptyMessage: "No messages yet. Start the conversation!",
  },
};

const manyMessages: ChatMessageData[] = Array.from({ length: 20 }, (_, i) => ({
  id: String(i + 1),
  type: i % 3 === 0 ? "system" : i % 2 === 0 ? "own" : "other",
  senderName: i % 2 === 1 ? "Alice" : undefined,
  content:
    i % 3 === 0
      ? `システムメッセージ ${i + 1}`
      : `メッセージ ${i + 1}。これはスクロールテスト用の長めのメッセージです。`,
  timestamp: now - (20 - i) * 60000,
})) as ChatMessageData[];

export const ManyMessages: Story = {
  args: {
    messages: manyMessages,
    renderMessage,
    autoScroll: true,
  },
};

export const NoAutoScroll: Story = {
  args: {
    messages: manyMessages,
    renderMessage,
    autoScroll: false,
  },
};

const day = 24 * 60 * 60 * 1000;

export const AcrossDays: Story = {
  args: {
    messages: [
      { id: "d1", type: "other", senderName: "Alice", content: "昨日の続きからやろう", timestamp: now - 2 * day },
      { id: "d2", type: "own", content: "了解", timestamp: now - 2 * day + 60000 },
      { id: "d3", type: "other", senderName: "Alice", content: "今日もよろしく", timestamp: now },
    ] as ChatMessageData[],
    renderMessage,
  },
};
