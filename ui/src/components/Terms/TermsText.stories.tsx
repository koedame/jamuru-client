import type { Meta, StoryObj } from "@storybook/react-vite";
import { TermsText } from "./TermsText";

const markdown = `# 利用規約

## 第 1 条（本サービスでできること）

1. 招待コードを知っている少人数が、**インターネット越しに**低い遅延で音声をやり取りします。
   - 音声は、利用者の端末どうしで直接届きます（P2P）。

---

## 第 2 条（使える人）

18 歳未満の方は、親権者（保護者）の同意を得てから使ってください。
`;

const license = `MIT License

Copyright (c) koedame

Permission is hereby granted, free of charge, to any person obtaining a copy of this software.`;

const meta = {
  title: "Components/Terms/TermsText",
  component: TermsText,
  parameters: { layout: "centered" },
  decorators: [
    (Story) => (
      <div style={{ width: "480px", background: "var(--color-bg-primary)", padding: "16px" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
} satisfies Meta<typeof TermsText>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Markdown: Story = { args: { text: markdown, format: "markdown" } };

export const MarkdownWithoutTitle: Story = { args: { text: markdown, format: "markdown", skipTitle: true } };

export const Plain: Story = { args: { text: license, format: "plain" } };
