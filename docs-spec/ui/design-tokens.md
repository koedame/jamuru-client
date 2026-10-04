# デザイントークン

jamuru UI で使用する CSS Custom Properties（CSS変数）の定義。

> **実装先**: `ui/src/styles/tokens.css`

---

## 概要

すべてのスタイリングは CSS 変数を通じて行う。
これにより、テーマ切り替え（ダーク/ライト）が容易になり、
デザインの一貫性が保たれる。

### 命名規則

```
--{category}-{property}-{variant}

例:
--color-bg-primary
--space-md
--radius-lg
```

---

## カラートークン

### ダークテーマ（デフォルト）

ui.pen（Pencil schema 2.14、2026-07-18更新）のブランド刷新に追従した配色。
`bg-page`（基調）< `bg-card`（カード、page より明るい）で階層を作り、`bg-row`（入力欄・行・
メーター背景）を最も暗い recessed 面として使う3階調 + `accent-blue` を主要アクション色とする。
`yellow-primary` はロゴ等ブランド要素専用に用途を限定する（前リビジョンまでのイエロー基調から
転換）。

```css
:root,
:root[data-theme="dark"] {
  /* === Background === */
  --color-bg-primary: #1C1C1E;      /* 画面背景（ui.pen: --bg-page） */
  --color-bg-secondary: #2C2C2E;    /* カード・ヘッダー背景（ui.pen: --bg-card。page より明るい） */
  --color-bg-tertiary: #141414;     /* 入力欄・行・メーター背景（ui.pen: --bg-row。最も暗い recessed 面） */
  --color-bg-elevated: #48484A;     /* ホバー時の浮き上がり（ui.pen: gray-600） */
  --color-bg-hover: #48484A;        /* ホバー背景（--color-bg-elevated と同値のエイリアス） */
  --color-bg-overlay: rgba(0, 0, 0, 0.6);  /* モーダル背景スクリム */
  --color-bg-error: rgba(255, 69, 58, 0.1); /* エラー状態の背景ハイライト（--color-danger のtint） */

  /* === Accent === */
  --color-accent: #0A84FF;          /* 主要アクション・フォーカス状態（ui.pen: --accent-blue） */
  --color-accent-hover: #3D9FFF;    /* ホバー時（明度を上げる） */
  --color-accent-active: #0A6ACC;   /* アクティブ時（明度を下げる） */
  --color-accent-secondary: #FF9F0A; /* セカンダリブランドカラー（ui.pen: --orange-secondary）。チャンネルのソロ機能は未実装のため現状未使用（--color-solo-active と共に将来の再実装に備え保持） */
  --color-brand: #FFD60A;           /* ブランドカラー（ui.pen: --yellow-primary）。ロゴ等ブランド要素専用、アクションには使わない */

  /* === Semantic === */
  --color-success: #30D158;         /* 接続成功、正常状態（ui.pen: --color-meter-low） */
  --color-success-bg: rgba(48, 209, 88, 0.1);
  --color-warning: #FFD60A;         /* 警告、不安定（ui.pen: --color-meter-mid） */
  --color-warning-bg: rgba(255, 214, 10, 0.1);
  --color-danger: #FF453A;          /* エラー、切断（ui.pen: --color-meter-high） */
  --color-danger-bg: rgba(255, 69, 58, 0.1);

  /* === Text === */
  --color-text-primary: #F5F5F7;    /* 主要テキスト（ui.pen: --text-light） */
  --color-text-secondary: #98989D;  /* 補助テキスト（ui.pen: gray-400） */
  --color-text-tertiary: #6E6E73;   /* 三次テキスト・アイコン（--color-text-disabled と同値） */
  --color-text-muted: #6E6E73;      /* 抑制テキスト（--color-text-disabled と同値のエイリアス） */
  --color-text-disabled: #6E6E73;   /* 無効状態・プレースホルダー（ui.pen: gray-500） */
  --color-text-inverse: #F5F5F7;    /* 反転テキスト（accent-blue/dangerボタン上の文字など。白に近い text-light を流用） */

  /* === Border === */
  --color-border: #38383A;          /* 通常ボーダー（ui.pen: --border-dark） */
  --color-border-default: #38383A;  /* --color-border のエイリアス */
  --color-border-subtle: #141414;   /* 目立たせないボーダー（--color-bg-tertiary と同値） */
  --color-border-focus: #0A84FF;    /* フォーカス時（--color-accent と同値） */
  --color-border-hover: #48484A;    /* ホバー時 */

  /* === Connection Status === */
  --color-status-disconnected: #6E6E73;   /* グレー */
  --color-status-connecting: #FFD60A;     /* 黄（点滅） */
  --color-status-connected: #30D158;      /* 緑 */
  --color-status-unstable: #FFD60A;       /* 黄 */
  --color-status-error: #FF453A;          /* 赤 */
  --color-status-success: #30D158;        /* --color-success のエイリアス */
  --color-status-warning: #FFD60A;        /* --color-warning のエイリアス */

  /* === Mixer === */
  --color-meter-low: #30D158;       /* -∞ to -12dB */
  --color-meter-mid: #FFD60A;       /* -12 to -3dB */
  --color-meter-high: #FF453A;      /* -3dB to 0dB */
  --color-mute-active: #FF453A;     /* ミュートON */
  --color-solo-active: #FF9F0A;     /* ソロON。チャンネルのソロ機能は未実装のため現状未使用 */
}
```

### ライトテーマ

ui.pen 側にライトモード仕様がまだ存在しないため、暫定的にダークテーマと同一の値を使う
（プロオーディオツールに多いダーク専用の方針）。
`data-theme="light"` を設定する CSS の仕組み自体は残しているが、アプリ内にテーマ切り替え
UI は存在しない（設定画面 General タブに元々あったテーマセレクターは、ui.pen の General
タブにテーマ操作がなく、かつライト/ダークの値が同一で見た目に差がないため削除した。
詳細はコンポーネント仕様 [settings-panel.md](./components/settings-panel.md) を参照）。
ui.pen にライトモード仕様が追加され次第、専用の配色に更新し、必要であれば切り替えUIを再設計する。

```css
:root[data-theme="light"] {
  /* ダークテーマと同一の値（上記参照） */
}
```

同じ値は `@media (prefers-color-scheme: light) { :root:not([data-theme]) { ... } }` にも
定義されており、`data-theme` 属性が未設定の場合（OS側がライトモードの場合）に適用される。

### レベルメーターのグラデーション塗り（例外的に許可）

「装飾的なグラデーション禁止」の原則に対し、レベルメーター（`StereoMeter`/`InputLevelMeter`）
の塗りのみ例外とする。閾値ごとの色の塗り分けではなく、`--color-meter-low` →
`--color-meter-mid` → `--color-meter-high` の縦方向グラデーションで音量レベルを連続的に表現する
（意味を伝える機能的な装飾として許可。ui.pen 全体の原則でも明記されている）。

```css
.meter-fill {
  background: linear-gradient(
    to top,
    var(--color-meter-low) 0%,
    var(--color-meter-mid) 70%,
    var(--color-meter-high) 100%
  );
}
```

---

## タイポグラフィトークン

フォントは `@fontsource/inter`、`@fontsource/roboto-mono` としてバンドルし、Google Fonts 等の
外部CDNには依存しない（オフライン起動時にも表示崩れが起きないようにするため）。
ui.pen の `font-heading`（Inter）/`font-mono`（Roboto Mono）に合わせ、見出し専用フォント
（旧 Space Grotesk）は廃止し、見出しも本文と同じ Inter を使う。

```css
:root {
  /* === Font Family === */
  --font-family-sans:
    'Inter',
    -apple-system,
    BlinkMacSystemFont,
    'Segoe UI',
    Roboto,
    'Hiragino Sans',
    'Hiragino Kaku Gothic ProN',
    'Noto Sans JP',
    sans-serif;

  /* 見出し用（ui.pen: --font-heading）。本文と同じ Inter を使い、太さ/サイズで差をつける */
  --font-family-heading:
    'Inter',
    var(--font-family-sans);

  --font-family-mono:
    'Roboto Mono',
    'SF Mono',
    'Fira Code',
    Consolas,
    'Courier New',
    monospace;

  /* === Font Size === */
  --font-size-h1: 24px;
  --font-size-h2: 18px;
  --font-size-h3: 16px;
  --font-size-body: 14px;
  --font-size-caption: 12px;
  --font-size-small: 11px;

  /* === Font Weight === */
  --font-weight-bold: 700;
  --font-weight-semibold: 600;
  --font-weight-normal: 400;

  /* === Line Height === */
  --line-height-tight: 1.25;
  --line-height-normal: 1.5;
  --line-height-relaxed: 1.75;

  /* === Letter Spacing === */
  --letter-spacing-tight: -0.02em;
  --letter-spacing-normal: 0;
  --letter-spacing-wide: 0.02em;
}
```

### タイポグラフィの使用例

```css
/* 見出し1 */
.h1 {
  font-family: var(--font-family-sans);
  font-size: var(--font-size-h1);
  font-weight: var(--font-weight-bold);
  line-height: var(--line-height-tight);
}

/* 本文 */
.body {
  font-family: var(--font-family-sans);
  font-size: var(--font-size-body);
  font-weight: var(--font-weight-normal);
  line-height: var(--line-height-normal);
}

/* 数値表示（遅延時間など） */
.numeric {
  font-family: var(--font-family-mono);
  font-size: var(--font-size-body);
  font-weight: var(--font-weight-semibold);
}
```

---

## スペーシングトークン

4px ベースのスペーシングシステム。

```css
:root {
  /* === Base Unit === */
  --space-unit: 4px;

  /* === Spacing Scale === */
  --space-xs: 4px;      /* 1 unit */
  --space-sm: 8px;      /* 2 units */
  --space-md: 16px;     /* 4 units */
  --space-lg: 24px;     /* 6 units */
  --space-xl: 32px;     /* 8 units */
  --space-2xl: 48px;    /* 12 units */
  --space-3xl: 64px;    /* 16 units */

  /* === Component Padding === */
  --padding-button: var(--space-sm) var(--space-md);
  --padding-card: var(--space-md);
  --padding-input: var(--space-sm) var(--space-sm);
  --padding-modal: var(--space-lg);

  /* === Layout === */
  --gap-xs: var(--space-xs);
  --gap-sm: var(--space-sm);
  --gap-md: var(--space-md);
  --gap-lg: var(--space-lg);
}
```

---

## 角丸トークン

ui.pen（2026-07-18改訂）は「角丸なし」の原則から、`radius-control`/`radius-card` の2段階
スケールに転換した。装飾目的の角丸は依然として避けるが、ボタン/入力/バッジ類とカード/パネル類の
2階層のみを許可する。

```css
:root {
  --radius-control: 8px;   /* ボタン・入力・バッジ・チップ（ui.pen: --radius-control） */
  --radius-card: 14px;     /* カード・パネル・モーダル（ui.pen: --radius-card） */
  --radius-full: 9999px;   /* 円形ボタン用（アイコンボタン等） */

  /* 旧スケールのエイリアス（既存コードとの互換のため保持。新規コードは上記2トークンを使う） */
  --radius-none: 0;
  --radius-sm: var(--radius-control);
  --radius-md: var(--radius-control);
  --radius-lg: var(--radius-card);
  --radius-xl: var(--radius-card);
}
```

### 使用ガイドライン

| コンポーネント | 角丸 |
|--------------|------|
| ボタン | `--radius-control` |
| 入力フィールド | `--radius-control` |
| バッジ・タグ | `--radius-control` |
| カード | `--radius-card` |
| モーダル・ダイアログ | `--radius-card` |
| アイコンボタン（円形） | `--radius-full` |
| フェーダーのつまみ（StereoFader thumb） | 例外的に `2px`（ui.pen上唯一2pxを使う箇所。トークン化せずハードコードで良い） |

---

## シャドウトークン

「シャドウなし」の原則から、フォーカスリングに加えボタンの浮き上がり・モーダルの浮遊感程度の
控えめなシャドウを許可する方針に転換した（過剰なシャドウ・装飾目的のシャドウは引き続き禁止）。

```css
:root {
  --shadow-button: 0 1px 3px rgba(0, 0, 0, 0.25);   /* Primary/Danger ボタンの浮き上がり */
  --shadow-modal: 0 4px 24px rgba(0, 0, 0, 0.5);    /* ダイアログ・トースト等の浮遊感 */
  --shadow-sm: var(--shadow-button);                /* 旧トークンのエイリアス */
  --shadow-md: var(--shadow-modal);
  --shadow-lg: var(--shadow-modal);
  --shadow-xl: var(--shadow-modal);

  /* Focus ring（accent-blue に統一） */
  --shadow-focus: 0 0 0 3px rgba(10, 132, 255, 0.4);
}
```

---

## アニメーショントークン

```css
:root {
  /* === Duration === */
  --duration-instant: 0ms;
  --duration-fast: 100ms;
  --duration-normal: 200ms;
  --duration-slow: 300ms;
  --duration-slower: 500ms;

  /* === Easing === */
  --ease-default: ease-out;
  --ease-in: ease-in;
  --ease-in-out: ease-in-out;
  --ease-bounce: cubic-bezier(0.68, -0.55, 0.265, 1.55);

  /* === Transition Presets === */
  --transition-fast: var(--duration-fast) var(--ease-default);
  --transition-normal: var(--duration-normal) var(--ease-default);
  --transition-slow: var(--duration-slow) var(--ease-default);
}

/* アクセシビリティ: motion 設定を尊重 */
@media (prefers-reduced-motion: reduce) {
  :root {
    --duration-instant: 0ms;
    --duration-fast: 0ms;
    --duration-normal: 0ms;
    --duration-slow: 0ms;
    --duration-slower: 0ms;
  }
}
```

### アニメーション使用ガイドライン

| シーン | Duration | 理由 |
|-------|----------|------|
| ボタンホバー | fast | 即時フィードバック |
| モーダル表示 | normal | スムーズな遷移 |
| ページ遷移 | slow | 視覚的区切り |
| 接続中スピナー | slower | 穏やかな回転 |

**注意**: 演奏中（セッションアクティブ時）はアニメーションを最小限に抑える。

---

## Z-Index トークン

```css
:root {
  --z-base: 0;
  --z-dropdown: 100;
  --z-sticky: 200;
  --z-overlay: 300;
  --z-modal: 400;
  --z-popover: 500;
  --z-tooltip: 600;
  --z-toast: 700;
}
```

---

## レスポンシブトークン

```css
:root {
  /* === Breakpoints === */
  --breakpoint-sm: 640px;   /* Mobile */
  --breakpoint-md: 768px;   /* Tablet */
  --breakpoint-lg: 1024px;  /* Desktop */
  --breakpoint-xl: 1280px;  /* Large Desktop */

  /* === Container === */
  --container-sm: 640px;
  --container-md: 768px;
  --container-lg: 1024px;
  --container-xl: 1200px;
}
```

### レスポンシブ使用例

```css
/* Mobile first */
.container {
  padding: var(--space-sm);
}

@media (min-width: 768px) {
  .container {
    padding: var(--space-md);
  }
}
```

---

## 実装チェックリスト

### ファイル作成

```bash
ui/src/styles/
├── tokens.css       # このドキュメントの CSS
├── reset.css        # CSS リセット
└── global.css       # グローバルスタイル（tokens.css をインポート）
```

### 使用規則

- [ ] 生のカラー値（`#FFD600`）を直接使用しない
- [ ] 生のピクセル値（`16px`）を直接使用しない（例外: ボーダー幅）
- [ ] テーマ切り替えは `data-theme` 属性で行う
- [ ] `prefers-reduced-motion` を尊重する

### テーマ切り替え実装（現状: 未使用）

現在、`data-theme` 属性を設定する呼び出し元はアプリ内に存在しない（設定画面のテーマ
セレクター削除に伴い、それを操作していた `useTheme` フックも削除済み）。ライト/ダーク
の値が同一なため、`data-theme` を設定しなくても `@media (prefers-color-scheme)` の
フォールバックで見た目は変わらない。将来、ui.pen にライトモード仕様が追加され配色が
分岐する時点で、以下のような実装を再導入することを想定する:

```typescript
// テーマ切り替え（再実装時の参考実装）
function setTheme(theme: 'dark' | 'light') {
  document.documentElement.setAttribute('data-theme', theme);
  localStorage.setItem('theme', theme);
}

// 初期化（システム設定を尊重）
function initTheme() {
  const saved = localStorage.getItem('theme');
  if (saved) {
    setTheme(saved as 'dark' | 'light');
  } else if (window.matchMedia('(prefers-color-scheme: light)').matches) {
    setTheme('light');
  } else {
    setTheme('dark');
  }
}
```
