# コンポーネントカタログ

jamjam で使用する UI コンポーネントの一覧と仕様。

---

## コンポーネント階層図

```
jamjam UI
├── ConnectionPanel          # 接続画面
│   ├── ConnectionHeader
│   ├── CreateRoomButton
│   ├── CodeInput
│   ├── JoinButton
│   ├── SettingsButton
│   ├── LoadingSpinner
│   ├── ErrorMessage
│   └── ConnectionHistory
│
├── MixerPanel               # ミキシングコンソール
│   ├── ChannelStrip
│   │   ├── StereoMeter
│   │   ├── StereoFader
│   │   ├── PanSlider
│   │   ├── MuteButton
│   │   └── AudioQualityBadge
│   └── MasterSection
│       ├── StereoMeter
│       ├── StereoFader
│       └── MuteButton
│
├── ChatPanel                # チャット
│   ├── ChatMessageList
│   │   └── ChatMessage
│   │       ├── ReactionBar
│   │       └── AddReactionButton
│   ├── ChatInput
│   └── EmojiPicker
│
├── SettingsPanel            # 設定画面
│   ├── VerticalTabs
│   ├── GeneralTab
│   ├── ProfileTab
│   ├── DevicesTab
│   └── DiagnosticsTab
│
├── SettingsHelp             # 設定の手伝い（ADR-044 §5）
│   ├── SettingsHelpQuestion
│   ├── SettingsHelpBar
│   └── SettingsHelpPanel（DevicesTab を再利用。手伝う人の窓 HelperScreen が使う）
│
├── SessionStats             # セッション統計（オーバーレイ）
│   ├── NetworkStats
│   ├── LatencyBreakdown
│   └── PeerAudioInfo
│
└── 共通コンポーネント
    ├── Button
    ├── Input
    ├── Select
    ├── FormField
    ├── ConnectionIndicator
    ├── Toast
    └── Modal
```

---

## コンポーネント一覧

### パネルコンポーネント（メイン）

| コンポーネント | 優先度 | ステータス | 仕様 |
|--------------|--------|----------|------|
| ConnectionPanel | P0 | 仕様作成済 | [connection-panel.md](./connection-panel.md) |
| MixerPanel | P0 | 仕様作成済 | [mixer-panel.md](./mixer-panel.md) |
| ChatPanel | P0 | 仕様作成済 | [chat-panel.md](./chat-panel.md) |
| SettingsPanel | P0 | 仕様作成済 | [settings-panel.md](./settings-panel.md) |
| SessionStats | P1 | 仕様作成済 | [session-stats.md](./session-stats.md) |
| SettingsHelp | P1 | 仕様作成済 | [settings-help.md](./settings-help.md) |

### 共通コンポーネント

| コンポーネント | 優先度 | ステータス | 仕様 |
|--------------|--------|----------|------|
| ConnectionIndicator | P0 | 仕様作成済 | [connection-indicator.md](./connection-indicator.md) |
| Button | P0 | 仕様未作成 | - |
| Input | P0 | 仕様未作成 | - |
| Select | P0 | 仕様未作成 | - |
| FormField | P1 | 仕様未作成 | - |
| Toast | P1 | 仕様作成済み（本ファイル内） | 下記「Toast」参照。`ui.pen` Toast States（success/error/info/warning）準拠 |
| Modal | P1 | 仕様作成済み（本ファイル内） | 下記「Modal」参照。退室確認は `ui.pen` Dialog/LeaveConfirm States 準拠 |

### ドメイン固有コンポーネント（パネル内部）

| コンポーネント | 親パネル | ステータス | 仕様 |
|--------------|---------|----------|------|
| StereoMeter | MixerPanel | mixer-panel.md内 | [mixer-panel.md#stereometer](./mixer-panel.md#stereometerステレオレベルメーター) |
| StereoFader | MixerPanel | mixer-panel.md内 | [mixer-panel.md#stereofader](./mixer-panel.md#stereofaderステレオ音量フェーダー) |
| PanSlider | MixerPanel | mixer-panel.md内 | [mixer-panel.md#panslider](./mixer-panel.md#pansliderパンスライダー) |
| AudioQualityBadge | MixerPanel | mixer-panel.md内 | [mixer-panel.md#audioqualitybadge](./mixer-panel.md#audioqualitybadge音質バッジ) |
| ChatMessage | ChatPanel | chat-panel.md内 | [chat-panel.md#chatmessage](./chat-panel.md#chatmessage個別メッセージ) |
| ChatInput | ChatPanel | chat-panel.md内 | [chat-panel.md#chatinput](./chat-panel.md#chatinput入力フィールド) |
| EmojiPicker | ChatPanel | chat-panel.md内 | [chat-panel.md#emojipicker](./chat-panel.md#emojipicker) |
| VerticalTabs | SettingsPanel | settings-panel.md内 | [settings-panel.md#verticaltabs](./settings-panel.md#verticaltabs垂直タブ) |
| ConnectionHistory | ConnectionPanel | connection-panel.md内 | [connection-panel.md#connectionhistory](./connection-panel.md#接続履歴connectionhistory) |

---

## コンポーネント仕様テンプレート

各コンポーネント仕様には以下を含める：

### 1. 概要
- 目的
- 使用場面

### 2. Props / API

```typescript
interface ComponentProps {
  required: string;        // 必須プロパティ
  optional?: string;       // オプション
  onEvent?: () => void;    // イベントハンドラ
}
```

### 3. 状態とバリアント

| バリアント | 用途 |
|-----------|------|
| primary | 主要アクション |
| secondary | 補助アクション |

### 4. ビジュアル仕様

```
┌─────────────────┐
│  [ボタン]       │
└─────────────────┘
```

### 5. アクセシビリティ

- ARIA属性
- キーボード操作
- スクリーンリーダー対応

### 6. i18n キー

使用する翻訳キーの一覧

### 7. 使用例

```tsx
<Component variant="primary" onClick={handleClick}>
  ラベル
</Component>
```

---

## 基本コンポーネント

### Button

**用途**: クリック可能なアクション

**バリアント**:
| バリアント | 用途 | 背景色 |
|-----------|------|--------|
| primary | 主要アクション（ルーム作成等） | `--color-accent` |
| secondary | 補助アクション（キャンセル等） | `--color-bg-elevated` |
| danger | 破壊的アクション（退出等） | `--color-danger` |
| ghost | テキストのみ | 透明 |

**サイズ**:
| サイズ | パディング | フォントサイズ |
|-------|-----------|---------------|
| sm | `--space-xs` `--space-sm` | `--font-size-caption` |
| md | `--space-sm` `--space-md` | `--font-size-body` |
| lg | `--space-md` `--space-lg` | `--font-size-h3` |

**状態**:
- default
- hover
- active
- disabled
- loading

---

### Input

**用途**: テキスト入力

**タイプ**:
- text: 通常のテキスト
- code: 招待コード入力（6文字、大文字変換）
- password: パスワード入力

**状態**:
- default
- focus
- error
- disabled

---

### Toast

**用途**: 一時的な通知メッセージ（`ui.pen` Toast States 準拠、幅280px、カード＋`--shadow-modal`）

**タイプ**:
| タイプ | 用途 | lucideアイコン | 例文 |
|-------|------|---------|--------|
| success | 成功通知 | `check` | 「コピーしました」 |
| error | エラー | `circle-alert` | 「エラーが発生しました」 |
| info | 情報 | `loader` | 「接続中...」 |
| warning | 警告 | `triangle-alert` | 「接続が不安定です」 |

背景・ボーダーはいずれも `--color-bg-secondary`（`bg-card`）+ `--radius-card`。アイコン色のみ
タイプごとに `--color-success`/`--color-danger`/`--color-accent`/`--color-warning` を使う。

**動作**:
- 右上に表示
- 3秒後に自動消去（エラーは手動消去のみ）
- 複数表示時はスタック

---

### Modal

**用途**: オーバーレイダイアログ

**注意**: 演奏中（セッションアクティブ時）はモーダル表示禁止。唯一の例外が退室確認ダイアログ
（`LeaveDialog`、下記）— 退室操作自体が意図的な確認を要するため。

**構造**:
```
┌────────────────────────────┐
│ [✕]               タイトル │
├────────────────────────────┤
│                            │
│     [ コンテンツ ]         │
│                            │
├────────────────────────────┤
│      [キャンセル] [OK]     │
└────────────────────────────┘
```

#### LeaveDialog（退室確認ダイアログ）

`ui.pen` Dialog/LeaveConfirm States 準拠。中央寄せ、幅180px、`--radius-card` +
`--shadow-modal`（blur 24px, y+4, `#00000080`）。`Screens/Main/LeaveDialogState` の
インライン簡易版レイアウト（画面内左下配置）は現行 `Screens/Main` との内容不一致があり
陳腐化しているため使用しない。States カタログを正とする。

| 状態 | 表示 |
|------|------|
| Default | 「本当に退室しますか？」+ Danger「退室する」ボタン + Secondary「キャンセル」ボタン |
| Loading | 「退室処理中...」+ Dangerボタンはスピナーアイコンのみ（不透明度50%）+ キャンセルは無効化（`--color-text-tertiary`、不透明度50%） |

キーボード操作: Escapeでキャンセル、Tabでフォーカストラップ（確認/キャンセルの2ボタン間を循環）。
既存実装（`MainScreen.tsx` 内蔵）のロジックをコンポーネントとして抽出し、上記ビジュアルに
合わせて再スタイルする。

**アクセシビリティ**:
- `role="dialog"`
- `aria-modal="true"`
- フォーカストラップ
- Escapeで閉じる

---

## ドメイン固有コンポーネント

### ConnectionIndicator

接続状態を色とアイコンで表示。

→ 詳細: [connection-indicator.md](./connection-indicator.md)

---

### MixerChannel

参加者ごとの音量コントロール。

**構成要素**:
- 参加者名
- レベルメーター（縦型）
- 音量フェーダー
- ミュートボタン
- ソロボタン（オプション）

**サイズ**:
- 幅: 80px
- 高さ: 親要素に合わせる

---

### PresetSelector

プリセット選択 UI。

**表示形式**: ラジオボタンリスト

**各項目**:
- ラジオボタン
- プリセット名（日本語）
- 説明文（1行）
- 推奨バッジ（該当する場合）

---

### LevelMeter

音声レベルをリアルタイム表示。

**向き**: 縦型（ミキサー用）、横型（インライン用）

**色分け**:
| レベル | 色 |
|-------|-----|
| -∞ to -12dB | `--color-meter-low` (緑) |
| -12 to -3dB | `--color-meter-mid` (黄) |
| -3 to 0dB | `--color-meter-high` (赤) |

**更新頻度**: 60fps（演奏中は30fpsに制限）

---

### VolumeSlider

音量調整用スライダー。

**向き**: 縦型（ミキサー）、横型（マスター）

**範囲**: 0% - 100%

**デフォルト**: 80%

**アクセシビリティ**:
- `role="slider"`
- `aria-valuemin="0"`
- `aria-valuemax="100"`
- `aria-valuenow="80"`
- `aria-valuetext="80パーセント"`

---

## 実装ガイドライン

### ファイル構成

```
ui/src/components/
├── Button/
│   ├── Button.tsx
│   ├── Button.css
│   └── index.ts
├── ConnectionIndicator/
│   ├── ConnectionIndicator.tsx
│   ├── ConnectionIndicator.css
│   └── index.ts
└── index.ts  # 全コンポーネントの再エクスポート
```

### 命名規則

- コンポーネント: PascalCase（`ConnectionIndicator`）
- ファイル: コンポーネント名と同じ
- CSS クラス: BEM または コンポーネントスコープ

### スタイリング

- CSS Custom Properties（design-tokens.md 参照）を使用
- コンポーネント固有のCSSは同ディレクトリに配置
- グローバルスタイルは `styles/` に配置
