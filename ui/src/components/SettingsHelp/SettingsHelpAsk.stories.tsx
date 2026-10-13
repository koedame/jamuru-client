import type { Meta, StoryObj } from "@storybook/react-vite";
import { SettingsHelpAsk } from "./SettingsHelpAsk";

const meta = {
  title: "Components/SettingsHelp/SettingsHelpAsk",
  component: SettingsHelpAsk,
  parameters: {
    layout: "fullscreen",
  },
  decorators: [
    (Story) => (
      <div style={{ position: "relative", width: "600px", height: "400px", background: "var(--color-bg-primary)" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof SettingsHelpAsk>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Asking for help with settings: what it means first, then who to ask */
export const Default: Story = {
  args: {
    title: "Ask for help with settings?",
    lines: [
      "Someone you choose will look at jamuru's settings with you and can change them.",
      "They can't see the rest of your computer.",
      "You can stop it at any time.",
    ],
    chooseTitle: "Who do you want to ask?",
    noneText: "No one in this room can help with settings right now.",
    candidates: [
      { id: "aki", name: "Aki" },
      { id: "bo", name: "Bo" },
    ],
    chooseLabel: "Choose who",
    askLabel: "Ask",
    cancelLabel: "Never mind",
    onAsk: () => {},
    onCancel: () => {},
  },
};

/** The same, in Japanese */
export const Japanese: Story = {
  args: {
    ...Default.args,
    title: "設定依頼をしますか？",
    lines: [
      "選んだ人が、jamuru の設定を一緒に見て、変えてくれます。",
      "パソコンの画面や jamuru 以外のものは見えません。",
      "いつでもあなたが止められます。",
    ],
    chooseTitle: "誰に頼みますか？",
    noneText: "いま、設定を手伝える人がルームにいません。",
    candidates: [
      { id: "aki", name: "あき" },
      { id: "bo", name: "ボブ" },
    ],
    chooseLabel: "相手を選ぶ",
    askLabel: "依頼する",
    cancelLabel: "やめる",
  },
};
