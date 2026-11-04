/**
 * Foundation/Design Tokens - every token in `styles/tokens.css`, drawn from the
 * file itself (see `designTokens.ts`). The CSS is the source of truth; this page
 * only shows it, so there is nothing here to keep in step by hand.
 */
import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { tokensOf, type Token, type TokenGroup } from "./designTokens";

const meta = {
  title: "Foundation/Design Tokens",
  parameters: { layout: "fullscreen" },
} satisfies Meta;

export default meta;
type Story = StoryObj;

const mono: CSSProperties = {
  fontFamily: "var(--font-family-mono)",
  fontSize: "var(--font-size-caption)",
  color: "var(--color-text-secondary)",
};

const checker: CSSProperties = {
  backgroundImage:
    "linear-gradient(45deg, #3a3a3c 25%, transparent 25%, transparent 75%, #3a3a3c 75%), linear-gradient(45deg, #3a3a3c 25%, transparent 25%, transparent 75%, #3a3a3c 75%)",
  backgroundSize: "12px 12px",
  backgroundPosition: "0 0, 6px 6px",
};

/** The value the browser resolves the token to (an alias shows what it points at) */
function useResolved(name: string): string {
  const [value, setValue] = useState("");
  useEffect(() => {
    setValue(getComputedStyle(document.documentElement).getPropertyValue(name).trim());
  }, [name]);
  return value;
}

function Row({ token, preview }: { token: Token; preview: ReactNode }) {
  const resolved = useResolved(token.name);
  return (
    <div
      data-testid={`token-${token.name}`}
      style={{
        display: "grid",
        gridTemplateColumns: "minmax(220px, 1fr) minmax(200px, 1.2fr) 2fr",
        alignItems: "center",
        gap: "var(--space-md)",
        padding: "var(--space-sm) 0",
        borderBottom: "1px solid var(--color-border)",
      }}
    >
      <code style={{ ...mono, color: "var(--color-text-primary)" }}>{token.name}</code>
      <span style={mono}>
        {token.declared}
        {resolved && resolved !== token.declared && (
          <span style={{ display: "block", color: "var(--color-text-tertiary)" }}>= {resolved}</span>
        )}
      </span>
      <div>{preview}</div>
    </div>
  );
}

function Page({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return (
    <div
      style={{
        padding: "var(--space-lg)",
        background: "var(--color-bg-primary)",
        color: "var(--color-text-primary)",
        minHeight: "100vh",
        fontFamily: "var(--font-family-sans)",
      }}
    >
      <h1 style={{ fontSize: "var(--font-size-h1)", margin: 0 }}>{title}</h1>
      {note && (
        <p style={{ color: "var(--color-text-secondary)", fontSize: "var(--font-size-body)" }}>{note}</p>
      )}
      {children}
    </div>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section style={{ marginTop: "var(--space-xl)" }}>
      <h2 style={{ fontSize: "var(--font-size-h2)", margin: "0 0 var(--space-sm)" }}>{title}</h2>
      {children}
    </section>
  );
}

function rowsOf(group: TokenGroup, preview: (token: Token) => ReactNode, filter?: (t: Token) => boolean) {
  return tokensOf(group)
    .filter((t) => (filter ? filter(t) : true))
    .map((t) => <Row key={t.name} token={t} preview={preview(t)} />);
}

const COLOR_SECTIONS: ReadonlyArray<readonly [string, string, (name: string) => boolean]> = [
  ["背景 (Background)", "bg", (n) => n.startsWith("--color-bg-")],
  ["アクセント・ブランド (Accent / Brand)", "accent", (n) => /^--color-(accent|brand)/.test(n)],
  ["意味の色 (Semantic)", "semantic", (n) => /^--color-(success|warning|danger)/.test(n)],
  ["文字 (Text)", "text", (n) => n.startsWith("--color-text-")],
  ["枠線 (Border)", "border", (n) => n.startsWith("--color-border")],
  ["接続状態 (Connection status)", "status", (n) => n.startsWith("--color-status-")],
  ["ミキサー (Mixer)", "mixer", (n) => /^--color-(meter|mute|solo)/.test(n)],
];

function ColorPreview({ token }: { token: Token }) {
  return (
    <div
      style={{
        ...checker,
        width: 120,
        height: 32,
        borderRadius: "var(--radius-control)",
        border: "1px solid var(--color-border)",
        overflow: "hidden",
      }}
    >
      <div style={{ width: "100%", height: "100%", background: `var(${token.name})` }} />
    </div>
  );
}

export const Colors: Story = {
  render: () => (
    <Page
      title="Colors"
      note="jamuru はダークのみのブランド。light テーマは dark と同じ値 (tokens.css)。透過のある色は市松の上に重ねている。"
    >
      {COLOR_SECTIONS.map(([title, id, match]) => (
        <Section key={id} title={title}>
          {rowsOf("color", (t) => <ColorPreview token={t} />, (t) => match(t.name))}
        </Section>
      ))}
    </Page>
  ),
};

const SAMPLE = "Aa あいう The quick brown fox 0123456789";

export const Typography: Story = {
  render: () => (
    <Page title="Typography" note="見出しも本文と同じ Inter。太さと大きさで差をつける。">
      <Section title="書体 (Font family)">
        {rowsOf("typography", (t) => (
          <span style={{ fontFamily: `var(${t.name})`, fontSize: "var(--font-size-h3)" }}>{SAMPLE}</span>
        ), (t) => t.name.startsWith("--font-family-"))}
      </Section>
      <Section title="大きさ (Font size)">
        {rowsOf("typography", (t) => <span style={{ fontSize: `var(${t.name})` }}>{SAMPLE}</span>, (t) =>
          t.name.startsWith("--font-size-")
        )}
      </Section>
      <Section title="太さ (Font weight)">
        {rowsOf("typography", (t) => <span style={{ fontWeight: `var(${t.name})` as never }}>{SAMPLE}</span>, (t) =>
          t.name.startsWith("--font-weight-")
        )}
      </Section>
      <Section title="行の高さ (Line height)">
        {rowsOf("typography", (t) => (
          <p style={{ margin: 0, lineHeight: `var(${t.name})`, maxWidth: 360, background: "var(--color-bg-secondary)" }}>
            {SAMPLE}. {SAMPLE}.
          </p>
        ), (t) => t.name.startsWith("--line-height-"))}
      </Section>
      <Section title="文字間 (Letter spacing)">
        {rowsOf("typography", (t) => <span style={{ letterSpacing: `var(${t.name})` }}>{SAMPLE}</span>, (t) =>
          t.name.startsWith("--letter-spacing-")
        )}
      </Section>
    </Page>
  ),
};

export const Spacing: Story = {
  render: () => (
    <Page title="Spacing" note="4px ベースのグリッド。gap と padding は space の別名。">
      <Section title="余白の段階 (Space)">
        {rowsOf("spacing", (t) => (
          <div style={{ width: `var(${t.name})`, height: 16, background: "var(--color-accent)" }} />
        ), (t) => t.name.startsWith("--space-"))}
      </Section>
      <Section title="間隔 (Gap)">
        {rowsOf("spacing", (t) => (
          <div style={{ display: "flex", gap: `var(${t.name})` }}>
            <div style={{ width: 24, height: 24, background: "var(--color-accent)" }} />
            <div style={{ width: 24, height: 24, background: "var(--color-accent)" }} />
          </div>
        ), (t) => t.name.startsWith("--gap-"))}
      </Section>
      <Section title="部品の内側 (Padding)">
        {rowsOf("spacing", (t) => (
          <div style={{ display: "inline-block", padding: `var(${t.name})`, background: "var(--color-accent)" }}>
            <div style={{ background: "var(--color-bg-primary)", padding: "2px 6px", fontSize: "var(--font-size-small)" }}>
              content
            </div>
          </div>
        ), (t) => t.name.startsWith("--padding-"))}
      </Section>
    </Page>
  ),
};

export const Radius: Story = {
  render: () => (
    <Page title="Radius" note="新しい CSS は --radius-control / --radius-card を使う。sm〜xl は互換の別名。">
      {rowsOf("radius", (t) => (
        <div
          style={{
            width: 96,
            height: 48,
            background: "var(--color-bg-secondary)",
            border: "1px solid var(--color-border-hover)",
            borderRadius: `var(${t.name})`,
          }}
        />
      ))}
    </Page>
  ),
};

export const Shadows: Story = {
  render: () => (
    <Page title="Shadows" note="影はフォーカスリング・ボタンの浮き・モーダルとトーストだけに使う。">
      {rowsOf("shadow", (t) => (
        <div
          style={{
            width: 120,
            height: 48,
            margin: "var(--space-sm)",
            background: "var(--color-bg-secondary)",
            borderRadius: "var(--radius-control)",
            boxShadow: `var(${t.name})`,
          }}
        />
      ))}
    </Page>
  ),
};

function MotionDemo({ transition }: { transition: string }) {
  const [moved, setMoved] = useState(false);
  return (
    <div
      onMouseEnter={() => setMoved(true)}
      onMouseLeave={() => setMoved(false)}
      style={{ width: 200, background: "var(--color-bg-tertiary)", borderRadius: "var(--radius-control)", padding: 4 }}
    >
      <div
        style={{
          width: 24,
          height: 24,
          borderRadius: "var(--radius-control)",
          background: "var(--color-accent)",
          transform: moved ? "translateX(168px)" : "translateX(0)",
          transition: `transform ${transition}`,
        }}
      />
    </div>
  );
}

export const Motion: Story = {
  render: () => (
    <Page
      title="Motion"
      note="枠の上にマウスを乗せると動く。OS の「視差効果を減らす」が有効だと duration は 0ms になる。"
    >
      <Section title="時間 (Duration)">
        {rowsOf("motion", (t) => <MotionDemo transition={`var(${t.name}) var(--ease-default)`} />, (t) =>
          t.name.startsWith("--duration-")
        )}
      </Section>
      <Section title="緩急 (Easing)">
        {rowsOf("motion", (t) => <MotionDemo transition={`var(--duration-slower) var(${t.name})`} />, (t) =>
          t.name.startsWith("--ease-")
        )}
      </Section>
      <Section title="組み合わせ (Transition)">
        {rowsOf("motion", (t) => <MotionDemo transition={`var(${t.name})`} />, (t) =>
          t.name.startsWith("--transition-")
        )}
      </Section>
    </Page>
  ),
};

export const Layers: Story = {
  render: () => (
    <Page title="Layers" note="z-index の段。上の行ほど手前。">
      {rowsOf("layer", (t) => (
        <div
          style={{
            width: `calc(40px + var(${t.name}) * 0.4)`,
            height: 12,
            background: "var(--color-accent)",
            borderRadius: "var(--radius-control)",
          }}
        />
      )).reverse()}
    </Page>
  ),
};

export const Layout: Story = {
  render: () => (
    <Page title="Layout" note="ブレークポイントとコンテナの幅。バーは値そのままの幅。">
      {rowsOf("layout", (t) => (
        <div
          style={{
            width: `var(${t.name})`,
            maxWidth: "100%",
            height: 12,
            background: "var(--color-accent)",
            borderRadius: "var(--radius-control)",
          }}
        />
      ))}
    </Page>
  ),
};
