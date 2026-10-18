# ConnectionIndicator コンポーネント

接続状態をリアルタイムで表示するコンポーネント。

---

## 概要

**目的**: ユーザーに現在の接続状態を一目で伝える

**使用場面**:
- コンポーネントカタログ（`Catalog.tsx`）
- 接続状態の汎用インジケータが必要な箇所

### 現在の実装での配置（ui.pen Screens/Main）

接続済み画面（`MainScreen.tsx`）では、大きな `ConnectionIndicator` は使用しない。
ui.pen の Screens/Main に合わせ、接続状態は 32px フッターのステータスドット
（`--color-success` の 8px 円）+ テキスト（「接続中」/`status.connected`）で表示し、
参加者ごとの ↑/↓ レイテンシはサイドバーの参加者行に表示する。
フッターには遅延の値を出さない（参加者が複数いると、どの人の値か分からないため）。自分のデバイスの入出力遅延、
ネットワークの往復などの内訳は、参加者の行を押して開く `PeerLinkDetail` に、その人との値として出す。
`ConnectionIndicator` コンポーネント自体は汎用インジケータとして保持する。

---

## Props / API

```typescript
interface ConnectionIndicatorProps {
  /** 接続状態 */
  status: ConnectionStatus;

  /** RTT（往復遅延）をミリ秒で表示 */
  latencyMs?: number;

  /** 遅延値を表示するか */
  showLatency?: boolean;

  /** サイズ */
  size?: 'sm' | 'md' | 'lg';

  /** クリック時のハンドラ（詳細表示へ遷移など） */
  onClick?: () => void;
}

type ConnectionStatus =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'unstable'
  | 'error';
```

### Props 詳細

| Prop | 型 | 必須 | デフォルト | 説明 |
|------|-----|------|-----------|------|
| status | ConnectionStatus | ✓ | - | 現在の接続状態 |
| latencyMs | number | - | undefined | RTT値（ms） |
| showLatency | boolean | - | true | 遅延値を表示するか |
| size | 'sm' \| 'md' \| 'lg' | - | 'md' | コンポーネントサイズ |
| onClick | function | - | undefined | クリックハンドラ |

---

## 状態とバリアント

### 接続状態

| Status | 色 | アイコン | テキスト(JA) | テキスト(EN) |
|--------|-----|---------|-------------|-------------|
| disconnected | `--color-status-disconnected` | ○ (空) | 未接続 | Disconnected |
| connecting | `--color-status-connecting` | ↻ (回転) | 接続中... | Connecting... |
| connected | `--color-status-connected` | ● (塗り) | 接続中 | Connected |
| unstable | `--color-status-unstable` | ⚠ | 不安定 | Unstable |
| error | `--color-status-error` | ✕ | 切断されました | Disconnected |

### サイズ

| Size | アイコンサイズ | フォントサイズ | 用途 |
|------|--------------|---------------|------|
| sm | 12px | `--font-size-caption` | ミキサーチャンネル |
| md | 16px | `--font-size-body` | メイン画面ヘッダー |
| lg | 20px | `--font-size-h3` | フルスクリーン表示 |

---

## ビジュアル仕様

### レイアウト

```
┌─────────────────────────────┐
│ [●] 接続中（15ms）          │
└─────────────────────────────┘
  ↑   ↑            ↑
  │   │            └─ 遅延値（latencyMsが設定時）
  │   └─ ステータステキスト
  └─ ステータスアイコン
```

### 各状態の表示例

**未接続 (disconnected)**
```
○ 未接続
```

**接続中 (connecting)**
```
↻ 接続中...   ← アイコンが回転アニメーション
```

**接続済み (connected)**
```
● 接続中（15ms）   ← 遅延値は表示設定による
```

**不安定 (unstable)**
```
⚠ 不安定（45ms）   ← 黄色で警告
```

**エラー (error)**
```
✕ 切断されました   ← 赤色
```

### 色の適用

```css
.connection-indicator {
  display: inline-flex;
  align-items: center;
  gap: var(--space-xs);
}

.connection-indicator__icon {
  width: 16px; /* size=md の場合 */
  height: 16px;
}

.connection-indicator--disconnected .connection-indicator__icon {
  color: var(--color-status-disconnected);
}

.connection-indicator--connecting .connection-indicator__icon {
  color: var(--color-status-connecting);
  animation: spin 1s linear infinite;
}

.connection-indicator--connected .connection-indicator__icon {
  color: var(--color-status-connected);
}

.connection-indicator--unstable .connection-indicator__icon {
  color: var(--color-status-unstable);
}

.connection-indicator--error .connection-indicator__icon {
  color: var(--color-status-error);
}

@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}
```

---

## アニメーション

### 接続中 (connecting)

```css
.connection-indicator--connecting .connection-indicator__icon {
  animation: spin 1s linear infinite;
}

/* 演奏中は回転を停止（CPU負荷軽減） */
.session-active .connection-indicator--connecting .connection-indicator__icon {
  animation: none;
}

@media (prefers-reduced-motion: reduce) {
  .connection-indicator--connecting .connection-indicator__icon {
    animation: none;
  }
}
```

### 状態変更時

```css
.connection-indicator__icon {
  transition: color var(--transition-normal);
}

.connection-indicator__text {
  transition: color var(--transition-normal);
}
```

---

## アクセシビリティ

### ARIA 属性

```tsx
<div
  role="status"
  aria-live="polite"
  aria-label={getAriaLabel(status, latencyMs)}
  className={`connection-indicator connection-indicator--${status}`}
>
  <span className="connection-indicator__icon" aria-hidden="true">
    {getIcon(status)}
  </span>
  <span className="connection-indicator__text">
    {getText(status, latencyMs)}
  </span>
</div>
```

### aria-label の構築

```typescript
function getAriaLabel(status: ConnectionStatus, latencyMs?: number): string {
  const statusLabels = {
    disconnected: '接続状態: 未接続',
    connecting: '接続状態: 接続中',
    connected: latencyMs
      ? `接続状態: 接続中、遅延${latencyMs}ミリ秒`
      : '接続状態: 接続中',
    unstable: latencyMs
      ? `接続状態: 不安定、遅延${latencyMs}ミリ秒`
      : '接続状態: 不安定',
    error: '接続状態: 切断されました',
  };
  return statusLabels[status];
}
```

### スクリーンリーダー

- `role="status"` で状態変更を通知
- `aria-live="polite"` で他の読み上げを邪魔しない
- 状態変更時にスクリーンリーダーが自動的に読み上げる

### キーボード操作

- `onClick` が設定されている場合、`tabindex="0"` を追加
- Enter/Space でクリックイベント発火

---

## i18n キー

```json
{
  "status": {
    "disconnected": "未接続",
    "connecting": "接続中...",
    "connected": "接続中",
    "unstable": "不安定",
    "error": "切断されました",
    "latency": "{{ms}}ms"
  }
}
```

### 使用例

```typescript
import { useTranslation } from 'react-i18next';

function ConnectionIndicator({ status, latencyMs }: Props) {
  const { t } = useTranslation();

  const text = t(`status.${status}`);
  const latency = latencyMs ? t('status.latency', { ms: latencyMs }) : '';

  return (
    <div className={`connection-indicator connection-indicator--${status}`}>
      {/* ... */}
    </div>
  );
}
```

---

## 使用例

### 基本使用

```tsx
<ConnectionIndicator status="connected" latencyMs={15} />
```

### 遅延非表示

```tsx
<ConnectionIndicator status="connected" showLatency={false} />
```

### ミキサーチャンネル（小サイズ）

```tsx
<ConnectionIndicator status="connected" latencyMs={15} size="sm" />
```

### クリック可能（詳細表示へ遷移）

```tsx
<ConnectionIndicator
  status="connected"
  latencyMs={15}
  onClick={() => navigate('/connection-info')}
/>
```

### 状態に応じた条件分岐

```tsx
function SessionHeader({ connectionStatus, latencyMs }: Props) {
  return (
    <header className="session-header">
      <ConnectionIndicator
        status={connectionStatus}
        latencyMs={connectionStatus === 'connected' ? latencyMs : undefined}
      />
      {/* ... */}
    </header>
  );
}
```

---

## テスト観点

### ユニットテスト

- [ ] 各 status で正しいアイコンと色が表示される
- [ ] latencyMs が設定されると遅延値が表示される
- [ ] showLatency=false で遅延値が非表示になる
- [ ] size に応じてサイズが変わる
- [ ] onClick が設定されるとクリック可能になる

### アクセシビリティテスト

- [ ] status 変更時に aria-label が更新される
- [ ] スクリーンリーダーで状態が読み上げられる
- [ ] キーボードでフォーカス・クリックできる

### ビジュアルリグレッションテスト

- [ ] 各 status のスナップショット
- [ ] 各 size のスナップショット
- [ ] ダーク/ライトテーマでの表示

---

## 実装ファイル構成

```
ui/src/components/ConnectionIndicator/
├── ConnectionIndicator.tsx       # コンポーネント本体
├── ConnectionIndicator.css       # スタイル
├── ConnectionIndicator.test.tsx  # テスト
├── icons.tsx                     # 各状態のアイコン
└── index.ts                      # エクスポート
```
