# jamuru UI 仕様書

Claude Code が UI 実装を効率的に進めるための仕様書インデックス。

> **Note**: このディレクトリは `ui-ux-guideline.md` を補完する。
> 既存の ui-ux-guideline.md は設計原則・用語・コンポーネント設計の概要を含む。
> このディレクトリでは、実装に必要な詳細仕様を提供する。

---

## ドキュメント構成

```
docs-spec/ui/
├── README.md                 # このファイル
├── design-guide.md           # デザインガイド（トンマナ）★重要
├── personas.md               # ユーザーペルソナ（詳細シナリオ付き）
├── glossary.md               # UI 用語集（文言の表記規則）
├── user-stories.md           # ユーザーストーリー（Epic/Story形式）
├── design-tokens.md          # デザイントークン（CSS変数）
├── i18n-strategy.md          # 国際化戦略
├── components/               # コンポーネント仕様
│   ├── README.md             # コンポーネントカタログ
│   └── connection-indicator.md
└── screens/                  # 画面仕様
    └── README.md             # 画面遷移図
```

---

## クイックリファレンス

### ペルソナ

| ペルソナ | 代表的ユースケース | 技術レベル |
|---------|-------------------|-----------|
| 山田太郎 | バンド練習 | 一般ユーザー |
| 鈴木花子 | 音楽レッスン | PC苦手 |
| 田中一郎 | プロセッション | 上級者 |

詳細: [personas.md](./personas.md)

### 設計判断の優先順位

1. **低遅延の維持** - UI がオーディオをブロックしない
2. **演奏中の安定性** - モーダル禁止、自動切り替え禁止
3. **1クリック開始** - 3ステップ以内でセッション開始
4. **専門用語の排除** - 一般ユーザーが理解できる言葉
5. **視線移動の最小化** - 重要情報は上部固定
6. **機能的なデザイン** - DAWライクなプロフェッショナル UI

詳細: [../ui-ux-guideline.md](../ui-ux-guideline.md) Section 1

### デザインの方向性（トンマナ）

**MixerPanel をベースとしたデザイン**

- ダークニュートラル基調 + `accent-blue` を主要アクションに積極使用
- 1px ボーダー、`radius-control`（8px）/`radius-card`（14px）の2段階角丸
- ボタンの浮き上がり・モーダルの浮遊感程度の控えめなシャドウを許可
- レベルメーターは緑→黄→赤のグラデーション塗り（意味を伝える例外的装飾）
- DAW/プロオーディオ機器のような機能美

詳細: [design-guide.md](./design-guide.md)

### 画面構成

```mermaid
graph LR
    A[メイン] --> B[ミキサー]
    A --> C[設定]
    A --> D[接続情報]
    E[オンボーディング] --> A
```

詳細: [screens/README.md](./screens/README.md)

### デザイントークン

スタイリングは CSS Custom Properties（CSS変数）を使用。

```css
/* 使用例 - MixerPanel スタイル */
.button {
  background: transparent;
  border: 1px solid var(--color-border);
  color: var(--color-text-primary);
  padding: var(--space-sm) var(--space-md);
}

.button--active {
  background: var(--color-text-primary);
  color: var(--color-bg-primary);
}
```

詳細: [design-tokens.md](./design-tokens.md) / [design-guide.md](./design-guide.md)

---

## 関連ドキュメント

| ドキュメント | 役割 |
|-------------|------|
| [ui-ux-guideline.md](../ui-ux-guideline.md) | 設計原則、用語、コンポーネント概要 |
| [architecture.md](../architecture.md) Section 8 | GUI フレームワーク決定 |
| [ADR-004](../adr/ADR-004-gui-framework.md) | Tauri 採用理由 |
| [ADR-007](../adr/ADR-007-i18n-library.md) | i18next 採用理由 |
| [ADR-008](../adr/ADR-008-zero-latency-mode.md) | ゼロレイテンシーモード |

---

## 実装時の注意事項

1. **ペルソナ「鈴木花子」を基準に設計する** - 最も技術に不慣れなユーザーで検証
2. **CSS変数は `ui/src/styles/tokens.css` に配置**
3. **i18n キーは `ui/locales/{ja,en}.json` に配置**
4. **コンポーネント実装前に対応する仕様書を確認**
