import type { Meta, StoryObj } from "@storybook/react-vite";
import { ConsentScreen } from "./ConsentScreen";
import { TermsViewer } from "./TermsViewer";

const terms = `# 利用規約

jamjam を使う方に、使い始める前に知っておいてほしいことを、この規約にまとめます。

## 第 1 条（本サービスでできること）

1. 招待コードを知っている少人数が、インターネット越しに低い遅延で音声をやり取りします。
2. 音声は、利用者の端末どうしで直接届きます（P2P）。

## 第 2 条（使える人）

1. **18 歳未満の方は、親権者（保護者）の同意を得てから使ってください。**
`;

const meta = {
  title: "Screens/Consent",
  component: ConsentScreen,
  parameters: { layout: "fullscreen" },
  tags: ["autodocs"],
  args: { text: terms, onAccept: () => {} },
} satisfies Meta<typeof ConsentScreen>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const Saving: Story = { args: { accepting: true } };

export const SaveFailed: Story = { args: { error: "disk full" } };

export const Viewer: StoryObj<typeof TermsViewer> = {
  render: () => <TermsViewer title="利用規約" text={terms} onClose={() => {}} />,
};
