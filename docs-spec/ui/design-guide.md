# デザインガイド - jamjam UI

jamjam のビジュアルデザイン方針。MixerPanel をトンマナのベースとする。

---

## デザイン原則

### 1. 機能優先（Function First）

```
デザインは機能に従う。装飾より操作性。
```

- 音楽制作ツール（DAW）のような専門的で機能的な美学
- 視覚的なノイズを排除し、本質的な情報のみ表示
- ユーザーが「操作できる」ことが即座にわかるUI

### 2. アクセントカラーの計画的使用（Deliberate Accent Use）

```
色は情報を伝える手段。だが主要アクションには積極的に使う。
```

- 基調はダークニュートラル（`bg-page`/`bg-card`/`bg-row` のグレースケール階調）
- `accent-blue` を主要アクション（プライマリボタン・フォーカスリング）に積極的に使用する
  （旧版の「モノクロームベース」原則から転換。前リビジョンの `.pen` 更新履歴を参照）
- `yellow-primary`（ブランドカラー）はロゴ等ブランド要素専用に限定し、アクションには使わない
- 意味のある色分けは引き続き維持:
  - 接続状態・レベルメーター（緑/黄/赤 = `color-meter-low`/`mid`/`high`）
  - エラー状態（赤）
  - フォーカス/主要アクション（`accent-blue`）

### 3. 精密さ（Precision）

```
1pxのズレも許容しない。
```

- グリッドベースの厳密なレイアウト
- 要素間のアライメント統一
- 数値は等幅フォントで桁揃え

---

## カラーパレット

### 基本色（常時使用）

| 用途 | トークン | 役割 |
|------|----------|------|
| 主要テキスト | `--color-text-primary` | メインのテキスト、アクティブな要素 |
| 副次テキスト | `--color-text-secondary` | 補助情報、非アクティブな要素 |
| 画面背景 | `--color-bg-primary` | メイン背景（`bg-page`） |
| カード背景 | `--color-bg-secondary` | カード・パネル背景（`bg-card`、画面背景より明るい） |
| 行・入力欄背景 | `--color-bg-tertiary` | 入力欄・メーター等の recessed 面（`bg-row`、最も暗い） |
| ボーダー | `--color-border` | 区切り線、境界 |
| 主要アクション | `--color-accent` | プライマリボタン、フォーカスリング（`accent-blue`） |
| ブランド | `--color-brand` | ロゴ等ブランド要素専用（`yellow-primary`）。アクションには使わない |

### 状態色（意味がある場合のみ）

| 状態 | トークン | 使用場面 |
|------|----------|----------|
| 接続成功 | `--color-success` | 接続インジケータ |
| 警告/不安定 | `--color-warning` | 接続不安定、注意 |
| エラー/ミュート | `--color-danger` | エラー、切断 |
| フォーカス | `--color-accent` | キーボードフォーカスリング |

### 使用禁止

- 装飾目的のグラデーション（レベルメーターの塗りのみ例外。[design-tokens.md](./design-tokens.md#レベルメーターのグラデーション塗り例外的に許可) 参照）
- ブランドカラー（`--color-brand`）をアクション・背景に使うこと
- 意味のない色分け

---

## タイポグラフィ

### フォント使い分け

| 種類 | フォント | 用途 |
|------|----------|------|
| UI テキスト | `--font-family-sans` | ラベル、説明文 |
| 数値 | `--font-family-mono` | dB表示、時間、サンプルレート |

### サイズ

| サイズ | トークン | 用途 |
|--------|----------|------|
| 11px | `--font-size-small` | メーター数値、品質表示 |
| 12px | `--font-size-caption` | 補助ラベル |
| 14px | `--font-size-body` | 通常テキスト |

### 数値表示の原則

```css
/* 数値は常に等幅フォント */
.numeric {
  font-family: var(--font-family-mono);
}

/* 右揃え or 中央揃えで桁を統一 */
.db-display {
  text-align: right;
  min-width: 4ch;  /* "-∞" から "+0.0" まで対応 */
}
```

---

## ボーダーと形状

### ボーダー

| 要素 | スタイル |
|------|----------|
| コンポーネント境界 | `1px solid var(--color-border)` |
| セクション区切り | `1px solid var(--color-border)` |
| インタラクティブ要素 | `1px solid var(--color-border)` |

### 角丸

2段階の角丸スケールを使う（詳細: [design-tokens.md#角丸トークン](./design-tokens.md#角丸トークン)）。
装飾目的の中途半端な角丸（このスケールに無い値）は禁止。

```css
/* ボタン・入力・バッジ */
border-radius: var(--radius-control);  /* 8px */

/* カード・パネル・モーダル */
border-radius: var(--radius-card);     /* 14px */

/* 円形ボタン */
border-radius: var(--radius-full);
```

### シャドウ

フォーカスリングに加え、ボタンの浮き上がり・モーダルの浮遊感程度の控えめなシャドウを許可する
（詳細: [design-tokens.md#シャドウトークン](./design-tokens.md#シャドウトークン)）。過剰な
シャドウ（大きなぼかし・複数レイヤー）は引き続き禁止。

```css
/* Primary/Danger ボタンの浮き上がり */
box-shadow: var(--shadow-button);

/* ダイアログ・トースト */
box-shadow: var(--shadow-modal);

/* フォーカスリング */
box-shadow: var(--shadow-focus);
```

---

## インタラクション

### 状態表現

| 状態 | 表現方法 |
|------|----------|
| デフォルト | `border: 1px solid var(--color-border)` |
| ホバー | `border-color: var(--color-text-primary)` |
| フォーカス | `box-shadow: var(--shadow-focus)` |
| アクティブ | 背景反転（text-primary ↔ bg-primary） |
| 無効 | `opacity: 0.5` |

### ボタン

種別ごとに塗り色で表現する（旧版のボーダーのみ+反転表現から転換）。

```css
/* Primary: 主要アクション */
.button--primary {
  background: var(--color-accent);
  color: var(--color-text-inverse);
  border-radius: var(--radius-control);
  box-shadow: var(--shadow-button);
}
.button--primary:hover { background: var(--color-accent-hover); }
.button--primary:active { background: var(--color-accent-active); }
.button--primary:focus-visible { box-shadow: var(--shadow-focus); }
.button--primary:disabled { opacity: 0.4; }

/* Secondary: 補助アクション */
.button--secondary {
  background: var(--color-bg-secondary);
  border: 1px solid var(--color-border);
  color: var(--color-text-primary);
  border-radius: var(--radius-control);
}

/* Danger: 破壊的アクション（退室等） */
.button--danger {
  background: var(--color-danger);
  color: var(--color-text-inverse);
  border-radius: var(--radius-control);
  box-shadow: var(--shadow-button);
}
```

### スライダー/フェーダー

```css
/* トラック */
.track {
  width: 1px;
  background: var(--color-text-primary);
}

/* サム（つまみ） */
.thumb {
  background: var(--color-bg-primary);
  border: 1px solid var(--color-text-primary);
}
```

---

## 状態のデザイン規則

デザイン（`ui.pen`）と Storybook のどちらでも、次の状態をすべて用意する。

### インタラクション状態

インタラクティブなコンポーネント（ボタン、入力、リンク等）は、下の状態のバリエーションをすべて用意する。

| 状態 | 説明 | 名前の末尾 | 用意するもの |
|------|------|-----------|-------------|
| Default | 通常状態 | `/Default` | すべて |
| Hover | マウスオーバー時 | `/Hover` | すべて |
| Pressed | クリック/タップ中 | `/Pressed` | すべて |
| Disabled | 操作不可 | `/Disabled` | すべて |
| Focus | キーボード操作の対象 | `/Focus` | キーボードで操作できる要素 |
| Active | トグルのオン状態 | `/Active` | トグル系 |
| Selected | 選択された状態 | `/Selected` | 選択できる要素 |
| Error | 検証に失敗した状態 | `/Error` | 入力の検証がある要素 |
| Loading | 非同期処理中 | `/Loading` | 非同期処理を待つ要素 |

- 名前は `{階層}/{カテゴリ}/{名前}/{状態}`（例: `Atoms/Button/Primary/Focus`）
- 同じコンポーネントの状態は横に並べ、間隔は 16px（`md`）にする。ほかのコンポーネントと区別できるよう、フレームにまとめる

### データ依存 UI の 5 状態（UI Stack）

リスト・パネル・フォームのようにデータを表示する UI は、次の 5 状態を考える。

| 状態 | 定義 | 表示するもの | 名前の末尾 |
|------|------|-------------|-----------|
| Empty | データが無い | 状況の説明と、次にやることへの誘導 | `/Empty` |
| Loading | データを取得中 | スケルトン UI かスピナー | `/Loading` |
| Partial | 一部だけ表示している | 取得済みのデータと、続きがあることの示唆（「もっと見る」・読み込み中の表示） | `/Partial` |
| Error | 取得や処理に失敗した | 原因を簡潔に。リトライなど回復の手段 | `/Error` |
| Ideal | 正常に表示している | 完全なデータ | `/Ideal`（付けなくてもよい） |

| コンポーネント | Empty | Loading | Partial | Error | Ideal |
|--------------|-------|---------|---------|-------|-------|
| ParticipantList | ✓ | ✓ | - | ✓ | ✓ |
| ChatPanel | ✓ | ✓ | ✓ | ✓ | ✓ |
| MixerPanel | ✓ | ✓ | - | ✓ | ✓ |
| ConnectionPanel | - | ✓ | - | ✓ | ✓ |

- デザインファイルでは各状態を縦に並べ、間隔は 24px（`lg`）にする。状態のラベルはデザインファイルにだけ書く

**状態ごとの原則**

- **Empty**: 責めるトーンを避ける（「何もありません」ではなく「〜はまだありません」。文言は [glossary.md](./glossary.md)）。次の一歩を示す。イラストやアイコンは控えめに
- **Loading**: 中身のレイアウトが予測できるスケルトンを優先する。動きはパルスやシマー程度。3 秒を超えそうなら進捗を出すことを検討する
- **Partial**: 取得済みのデータは完全に表示する。続きがあることを示す。無限スクロールなら読み込み中のインジケーターを出す
- **Error**: 技術的な詳細を避け、利用者の視点で説明する。回復の手段を必ず示す。赤は控えめに（アイコンとボーダーだけ）

---

## レイアウト

### スペーシング

4px ベースのグリッドシステム。

```css
--space-xs: 4px;   /* 最小間隔 */
--space-sm: 8px;   /* コンポーネント内部 */
--space-md: 16px;  /* コンポーネント間 */
```

### アライメント

```
┌─────────────────────────────────────┐
│  ラベル列  │  コンテンツ列         │
│  (右揃え)  │  (左揃え/中央揃え)    │
├───────────┼───────────────────────┤
│      音質 │  48kHz/2ch            │
│       PAN │  [====●====]          │
│      音量 │  ┃█████┃              │
│      名前 │  Alice                │
│    ミュート│  [🔊]                 │
└───────────┴───────────────────────┘
```

---

## コンポーネント例

### MixerPanel（リファレンス）

```
┌─────────────────────────────────────────────┐
│ ミキサー                                     │
├─────────────────────────────────────────────┤
│       │ 48kHz/2ch │ 48kHz/2ch │   48kHz/2ch │
│       │    C      │   L30     │             │
│       │ [══●══]   │ [●════]   │   -12db     │
│       │           │           │   -15db     │
│       │ +0.0 -14  │ -0.6 -13  │             │
│       │   ██      │   ██      │     ██      │
│       │   ██      │   ██      │     ██      │
│       │   ██      │   ██      │     ██      │
│       │   自分    │   Alice   │   マスター   │
│       │   [🎤]    │   [🔊]    │     [🔊]    │
└─────────────────────────────────────────────┘
```

### 特徴

1. **ヘッダー**: タブ形式、下線で区切り
2. **ラベル列**: 右揃え、副次テキスト色
3. **コントロール**: 1px ボーダー、`--radius-control`（8px）角丸
4. **メーター**: 緑→黄→赤のグラデーション塗り、ミュート時はグレー
5. **ボタン**: 種別ごとの塗り色（Primary=accent-blue、Danger=color-meter-high）でON/OFF表現

---

## 禁止事項

### やってはいけないこと

1. **装飾的なグラデーション**（レベルメーターの塗りのみ例外）
   ```css
   /* NG: レベルメーター以外での装飾目的のグラデーション */
   background: linear-gradient(to bottom, #7C5CFF, #5A3FD0);
   ```

2. **スケール外の角丸**（`--radius-control`/`--radius-card`/`--radius-full` 以外の値）
   ```css
   /* NG */
   border-radius: 5px;  /* トークンに存在しない中途半端な値 */
   ```

3. **過剰なシャドウ**（`--shadow-button`/`--shadow-modal`/`--shadow-focus` 以外の重厚なシャドウ）
   ```css
   /* NG */
   box-shadow: 0 10px 40px rgba(0, 0, 0, 0.5), 0 2px 8px rgba(0, 0, 0, 0.3);
   ```

4. **`bg-page`/`bg-card`/`bg-row` 以外の背景色でのグルーピング**
   （3階調の使い分けで階層を表現する。任意の背景色を増やして領域を分けることは禁止）
   ```css
   /* NG: トークンに無い背景色を独自に増やす */
   .section-a { background: #2A2A3E; }
   .section-b { background: #363652; }
   ```

5. **アイコンの多用**
   - テキストで表現できるものはテキストで
   - アイコンは「ミュート」「スピーカー」など普遍的なもののみ

---

## 実装チェックリスト

新しいコンポーネントを作成する際のチェック項目:

- [ ] 生の16進カラー値を直接使わず、トークン（`--color-*`）を使用しているか
- [ ] 状態色は意味がある場合のみ使用しているか
- [ ] `border-radius` は `--radius-control`/`--radius-card`/`--radius-full` のいずれかか
- [ ] `box-shadow` は `--shadow-button`/`--shadow-modal`/`--shadow-focus` のいずれかか
- [ ] 数値表示は `--font-family-mono` を使用しているか
- [ ] スペーシングは 4px の倍数か
- [ ] Default/Hover/Pressed/Disabled を用意し、該当すれば Focus/Active/Selected/Error/Loading も用意したか
- [ ] データを表示する UI は Empty/Loading/Error/Ideal を用意し、Partial が要るかを検討したか（各状態にメッセージと次の操作があるか）
- [ ] 状態のバリエーションは命名規則に従い、横に並べているか

---

## 参考

- MixerPanel コンポーネント: `ui/src/components/MixerPanel/`
- デザイントークン: `ui/src/styles/tokens.css`
