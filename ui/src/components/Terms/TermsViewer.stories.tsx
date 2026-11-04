import type { Meta, StoryObj } from "@storybook/react-vite";
import { TermsViewer } from "./TermsViewer";

const terms = `# 利用規約

## 第 1 条（本サービスでできること）

1. 招待コードを知っている少人数が、インターネット越しに低い遅延で音声をやり取りします。
2. 音声は、利用者の端末どうしで直接届きます（P2P）。
`;

const meta = {
  title: "Components/Terms/TermsViewer",
  component: TermsViewer,
  parameters: { layout: "fullscreen" },
  tags: ["autodocs"],
  args: { title: "利用規約", text: terms, format: "markdown", onClose: () => {} },
} satisfies Meta<typeof TermsViewer>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Terms: Story = {};

export const License: Story = {
  args: { title: "ライセンス", text: "MIT License\n\nCopyright (c) koedame", format: "plain" },
};
