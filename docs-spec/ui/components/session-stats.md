# SessionStats - セッション統計情報表示

ネットワーク状態と遅延の詳細情報を表示するコンポーネント。
セッション中のパフォーマンス監視とトラブルシューティングに使用する。

---

## 概要

### 目的
- ネットワーク品質（RTT、ジッター、パケットロス）をリアルタイム表示
- 遅延の内訳（upstream/downstream）を可視化
- オーディオバッファのアンダーラン状況を監視
- ピア側の音声設定とリサンプリング状況を確認

### 使用場面
- トラブルシューティング時の診断
- レイテンシ最適化時の確認

### 現在の実装での配置（ui.pen Screens/Main）

ui.pen の Screens/Main（接続済み画面）には詳細統計パネルが存在しないため、
`SessionStats` は接続済み画面には配置しない。接続済み画面では、レイテンシは
サイドバーの参加者行に ↑（upstream）/↓（downstream）として、接続状態は
フッターのステータスドット + テキスト（「接続中」）として表示する。

### 参加者行のレイテンシ表示

接続済み画面のサイドバーで、自分以外の参加者の行の右端に出す（`ui/src/screens/MainScreen.tsx`）。

- **値**: `↑` は自分から相手への片道遅延、`↓` は相手から自分への片道遅延。P2P なので方向ごとに別の値を持つ。整数の ms で、小数点は付けない
- **形式**: `↑8ms ↓9ms`。フォントは `--font-family-mono`、サイズは `--font-size-tiny`（10px）。矢印は `--color-text-tertiary`、数値は `--color-text-secondary`
- **色**: 数値の色は状態で変えない。遅延が大きいかどうかの判断は詳細統計（このコンポーネント）と診断に任せる
- **自分の行**: レイテンシは出さない（自分への遅延に意味がない）。行には自分の表示名を出す
- **値が無いとき**: 遅延の内訳をまだ取得できていないあいだは、行にレイテンシを出さない

`SessionStats` コンポーネントと Storybook ストーリーは、将来的な再利用
（例: 設定画面の診断タブ、別ウィンドウ）に備えて保持する。RTT・ジッター・
パケットロス・アンダーラン率の詳細表示が必要な場合はこのコンポーネントを使用する。

---

## Props / API

```typescript
interface SessionStatsProps {
  /** ネットワーク統計情報 */
  network: NetworkStats | null;
  /** 遅延詳細情報 */
  latency: DetailedLatency | null;
  /** アンダーラン率（0.0-1.0） */
  underrunRate?: number;
  /** ピア側のオーディオ情報 */
  peerAudio?: PeerAudioInfo | null;
  /** ローカルのサンプリングレート（Hz） */
  localSampleRate?: number;
}

interface NetworkStats {
  /** RTT（往復遅延）ミリ秒 */
  rtt_ms: number;
  /** ジッター（遅延揺らぎ）ミリ秒 */
  jitter_ms: number;
  /** パケットロス率（0.0-100.0） */
  packet_loss_percent: number;
  /** 送信パケット数 */
  packets_sent: number;
  /** 受信パケット数 */
  packets_received: number;
  /** 送信バイト数 */
  bytes_sent: bigint;
  /** 受信バイト数 */
  bytes_received: bigint;
}

interface DetailedLatency {
  /** アップストリーム（送信側）の遅延コンポーネント */
  upstream: LatencyComponent[];
  /** ダウンストリーム（受信側）の遅延コンポーネント */
  downstream: LatencyComponent[];
  /** アップストリーム合計（ミリ秒） */
  upstream_total_ms: number;
  /** ダウンストリーム合計（ミリ秒） */
  downstream_total_ms: number;
  /** 往復合計（ミリ秒） */
  roundtrip_total_ms: number;
}

interface LatencyComponent {
  /** コンポーネント名 */
  name: string;
  /** 遅延値（ミリ秒） */
  latency_ms: number;
}

interface PeerAudioInfo {
  /** ピア側のサンプリングレート（Hz） */
  sample_rate: number;
  /** ピア側のフレームサイズ（サンプル数） */
  frame_size: number;
  /** 使用中のコーデック */
  codec: string;
  /** リサンプリングが必要か */
  needs_resampling: boolean;
}
```

### Props 詳細

| Prop | 型 | 必須 | デフォルト | 説明 |
|------|-----|------|-----------|------|
| network | NetworkStats \| null | ✓ | - | ネットワーク統計（null で非表示） |
| latency | DetailedLatency \| null | ✓ | - | 遅延情報（null で非表示） |
| underrunRate | number | - | undefined | アンダーラン率（0.0-1.0） |
| peerAudio | PeerAudioInfo \| null | - | undefined | ピア側オーディオ情報 |
| localSampleRate | number | - | undefined | ローカルのサンプリングレート |

---

## ビジュアル仕様

### 全体レイアウト

```
┌──────────────────────────────────────────────────────┐
│ セッション統計                                       │
├──────────────────────────────────────────────────────┤
│ ネットワーク                                         │
│ ┌─────────────────┬─────────────────┬──────────────┐│
│ │ RTT             │ Jitter          │ Packet Loss  ││
│ │ 15 ms           │ 2 ms            │ 0.1%         ││
│ └─────────────────┴─────────────────┴──────────────┘│
│ ┌─────────────────────────────────────────────────┐ │
│ │ Underruns: 0.5%                                 │ │
│ │ ⚠ Buffer underruns detected                    │ │
│ └─────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────┤
│ ピア音声設定                                         │
│ ┌─────────────────┬─────────────────┬──────────────┐│
│ │ Sample Rate     │ Frame Size      │ Codec        ││
│ │ 48000 Hz        │ 480 samples     │ Opus         ││
│ └─────────────────┴─────────────────┴──────────────┘│
│ ┌─────────────────────────────────────────────────┐ │
│ │ ⚠ Resampling: 44100 Hz → 48000 Hz             │ │
│ │   (adds ~2ms latency)                           │ │
│ └─────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────┤
│ レイテンシ内訳                                       │
│ Upstream (送信)                                      │
│ ┌─────────────────────────────────────────────────┐ │
│ │ Capture          2.0 ms  ████                   │ │
│ │ Processing       0.5 ms  █                      │ │
│ │ Encoding         1.0 ms  ██                     │ │
│ │ Network          7.5 ms  ███████████            │ │
│ └─────────────────────────────────────────────────┘ │
│ Downstream (受信)                                    │
│ ┌─────────────────────────────────────────────────┐ │
│ │ Network          7.5 ms  ███████████            │ │
│ │ Jitter Buffer    5.0 ms  ███████                │ │
│ │ Decoding         1.0 ms  ██                     │ │
│ │ Processing       0.5 ms  █                      │ │
│ │ Playback         2.0 ms  ████                   │ │
│ └─────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────┤
│ 合計レイテンシ                                       │
│ ┌──────────────┬──────────────┬───────────────────┐ │
│ │ Upstream     │ Downstream   │ Roundtrip         │ │
│ │ 11.0 ms      │ 16.0 ms      │ 27.0 ms          │ │
│ └──────────────┴──────────────┴───────────────────┘ │
├──────────────────────────────────────────────────────┤
│ パケット統計                                         │
│ ┌──────────────────────────────────────────────────┐│
│ │ Sent: 15,234 packets (1.2 MB)                   ││
│ │ Received: 15,120 packets (1.1 MB)               ││
│ └──────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────┘
```

---

## セクション詳細

### 1. ネットワークセクション

**表示項目**:
- **RTT**: `{network.rtt_ms} ms`
- **Jitter**: `{network.jitter_ms} ms`
- **Packet Loss**: `{network.packet_loss_percent}%`
- **Underruns**: `{underrunRate * 100}%` (optional)

**警告表示**:
```typescript
// underrunRate > 0.005 (0.5%) で警告表示
if (underrunRate && underrunRate > 0.005) {
  return (
    <div className="session-stats__warning">
      ⚠ Buffer underruns detected
    </div>
  );
}
```

**色分け**:
| 項目 | 良好 | 注意 | 警告 |
|------|------|------|------|
| RTT | < 30ms | 30-50ms | > 50ms |
| Jitter | < 5ms | 5-10ms | > 10ms |
| Packet Loss | < 0.5% | 0.5-2% | > 2% |
| Underruns | < 0.5% | 0.5-1% | > 1% |

---

### 2. ピア音声設定セクション

**表示項目**:
- **Sample Rate**: `{peerAudio.sample_rate} Hz`
- **Frame Size**: `{peerAudio.frame_size} samples`
- **Codec**: `{peerAudio.codec}`

**リサンプリング警告**:
```typescript
if (peerAudio?.needs_resampling && localSampleRate) {
  return (
    <div className="session-stats__warning">
      ⚠ Resampling: {peerAudio.sample_rate} Hz → {localSampleRate} Hz
      <br />
      (adds ~2ms latency)
    </div>
  );
}
```

**表示条件**:
- `peerAudio` が null の場合はセクション全体を非表示

---

### 3. レイテンシ内訳セクション

**Upstream（送信側）**:
```typescript
{latency.upstream.map((component) => (
  <LatencyBar
    name={component.name}
    latency_ms={component.latency_ms}
    maxLatency={latency.upstream_total_ms}
  />
))}
```

**Downstream（受信側）**:
```typescript
{latency.downstream.map((component) => (
  <LatencyBar
    name={component.name}
    latency_ms={component.latency_ms}
    maxLatency={latency.downstream_total_ms}
  />
))}
```

**LatencyBar コンポーネント**:
```
┌─────────────────────────────────────────────────┐
│ Capture          2.0 ms  ████                   │
└─────────────────────────────────────────────────┘
  ↑                ↑       ↑
  名前              値      バー（相対幅）
```

**バー幅計算**:
```typescript
const barWidth = (latency_ms / maxLatency) * 100;
```

---

### 4. 合計レイテンシセクション

```
┌──────────────┬──────────────┬───────────────────┐
│ Upstream     │ Downstream   │ Roundtrip         │
│ 11.0 ms      │ 16.0 ms      │ 27.0 ms          │
└──────────────┴──────────────┴───────────────────┘
```

**データソース**:
- Upstream: `latency.upstream_total_ms`
- Downstream: `latency.downstream_total_ms`
- Roundtrip: `latency.roundtrip_total_ms`

**色分け**:
| 項目 | 良好 | 注意 | 警告 |
|------|------|------|------|
| Upstream | < 10ms | 10-15ms | > 15ms |
| Downstream | < 15ms | 15-25ms | > 25ms |
| Roundtrip | < 30ms | 30-50ms | > 50ms |

---

### 5. パケット統計セクション

```
Sent: 15,234 packets (1.2 MB)
Received: 15,120 packets (1.1 MB)
```

**フォーマット**:
```typescript
function formatBytes(bytes: bigint): string {
  const kb = Number(bytes) / 1024;
  if (kb < 1024) return `${kb.toFixed(1)} KB`;
  const mb = kb / 1024;
  return `${mb.toFixed(1)} MB`;
}

const sentText = `${network.packets_sent.toLocaleString()} packets (${formatBytes(network.bytes_sent)})`;
const receivedText = `${network.packets_received.toLocaleString()} packets (${formatBytes(network.bytes_received)})`;
```

---

## 警告インジケーター

### アンダーラン警告

**表示条件**:
```typescript
underrunRate && underrunRate > 0.005
```

**スタイル**:
```css
.session-stats__warning {
  background-color: var(--color-warning-bg);
  border-left: 3px solid var(--color-warning);
  padding: var(--space-sm);
  margin-top: var(--space-xs);
}
```

### リサンプリング警告

**表示条件**:
```typescript
peerAudio?.needs_resampling === true
```

**メッセージ**:
```
⚠ Resampling: {peer_rate} Hz → {local_rate} Hz
  (adds ~2ms latency)
```

---

## スタイル仕様

### カラートークン

固有トークンは定義せず、`design-tokens.md` の既存トークンのみを使用する
（`ui/src/components/SessionStats.css` の実装に準拠）:

```css
.session-stats__value--highlight { color: var(--color-success); }  /* 良好 */
.session-stats__value--warning   { color: var(--color-warning, #f59e0b); }  /* 注意/警告 */
```

- 良好: `--color-success`
- 注意・警告: `--color-warning`（現状 CSS 側は 2 段階のみ実装。上記「色分け」表の
  3段階目 = 「警告」相当を独立した色で区別する UI は未実装で、`--color-danger` の
  適用が候補）
- 背景・ボーダー・テキスト階調: `--color-bg-secondary`/`--color-bg-tertiary`/
  `--color-border`/`--color-text-primary`/`--color-text-secondary`/`--color-text-tertiary`
  （いずれも design-tokens.md 記載の既存トークン）

### レスポンシブ

| ブレークポイント | レイアウト |
|----------------|-----------|
| < 600px | 1カラム（縦積み） |
| 600-900px | 2カラム（Grid） |
| > 900px | 3カラム（Network統計） |

```css
.session-stats__network-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
  gap: var(--space-md);
}
```

---

## アクセシビリティ

### ARIA属性

```tsx
<section
  role="region"
  aria-label={t("sessionStats.title", "Session Statistics")}
  className="session-stats"
>
  <div role="group" aria-label={t("sessionStats.network", "Network")}>
    {/* ネットワーク統計 */}
  </div>

  <div role="group" aria-label={t("sessionStats.latency", "Latency Breakdown")}>
    {/* レイテンシ内訳 */}
  </div>
</section>
```

### スクリーンリーダー

**警告の読み上げ**:
```tsx
<div
  role="alert"
  aria-live="polite"
  className="session-stats__warning"
>
  {warningMessage}
</div>
```

### キーボードナビゲーション

- セクション間: Tab キー
- 折りたたみ可能な場合: Enter/Space でトグル

---

## i18n キー

```json
{
  "sessionStats": {
    "title": "セッション統計",
    "network": "ネットワーク",
    "rtt": "RTT",
    "jitter": "Jitter",
    "packetLoss": "Packet Loss",
    "underruns": "Underruns",
    "underrunWarning": "Buffer underruns detected",
    "peerAudio": "ピア音声設定",
    "sampleRate": "Sample Rate",
    "frameSize": "Frame Size",
    "codec": "Codec",
    "resamplingWarning": "Resampling: {{fromRate}} Hz → {{toRate}} Hz (adds ~2ms latency)",
    "latencyBreakdown": "レイテンシ内訳",
    "upstream": "Upstream (送信)",
    "downstream": "Downstream (受信)",
    "summary": "合計レイテンシ",
    "upstreamTotal": "Upstream",
    "downstreamTotal": "Downstream",
    "roundtripTotal": "Roundtrip",
    "packetStats": "パケット統計",
    "sent": "Sent: {{count}} packets ({{size}})",
    "received": "Received: {{count}} packets ({{size}})",
    "units": {
      "ms": "ms",
      "hz": "Hz",
      "samples": "samples",
      "percent": "%"
    }
  }
}
```

---

## 使用例

### 基本使用

```tsx
<SessionStats
  network={{
    rtt_ms: 15,
    jitter_ms: 2,
    packet_loss_percent: 0.1,
    packets_sent: 15234,
    packets_received: 15120,
    bytes_sent: 1258291n,
    bytes_received: 1153024n,
  }}
  latency={{
    upstream: [
      { name: "Capture", latency_ms: 2.0 },
      { name: "Processing", latency_ms: 0.5 },
      { name: "Encoding", latency_ms: 1.0 },
      { name: "Network", latency_ms: 7.5 },
    ],
    downstream: [
      { name: "Network", latency_ms: 7.5 },
      { name: "Jitter Buffer", latency_ms: 5.0 },
      { name: "Decoding", latency_ms: 1.0 },
      { name: "Processing", latency_ms: 0.5 },
      { name: "Playback", latency_ms: 2.0 },
    ],
    upstream_total_ms: 11.0,
    downstream_total_ms: 16.0,
    roundtrip_total_ms: 27.0,
  }}
/>
```

### アンダーラン警告付き

```tsx
<SessionStats
  network={networkStats}
  latency={latencyData}
  underrunRate={0.012} // 1.2% - 警告表示
/>
```

### ピア音声情報とリサンプリング警告

```tsx
<SessionStats
  network={networkStats}
  latency={latencyData}
  peerAudio={{
    sample_rate: 44100,
    frame_size: 441,
    codec: "Opus",
    needs_resampling: true,
  }}
  localSampleRate={48000}
/>
```

### データなし（ローディング状態）

```tsx
<SessionStats
  network={null}
  latency={null}
/>
// → "統計データを取得中..." 表示
```

---

## テスト観点

### ユニットテスト

- [ ] network が null の場合、ネットワークセクションが非表示になる
- [ ] latency が null の場合、遅延セクションが非表示になる
- [ ] underrunRate > 0.005 で警告が表示される
- [ ] needs_resampling = true でリサンプリング警告が表示される
- [ ] バイト数のフォーマットが正しい（KB/MB）
- [ ] 遅延バーの幅が相対的に正しい

### アクセシビリティテスト

- [ ] セクションに適切な role 属性がある
- [ ] 警告に role="alert" が設定される
- [ ] スクリーンリーダーで全情報が読み上げられる

### ビジュアルリグレッションテスト

- [ ] 全データ表示のスナップショット
- [ ] 警告表示のスナップショット
- [ ] データなし状態のスナップショット
- [ ] レスポンシブレイアウトのスナップショット

### パフォーマンステスト

- [ ] 統計データ更新時の再レンダリング回数
- [ ] 1秒間隔のポーリングでメモリリークしない

---

## 実装ファイル構成

```
ui/src/components/SessionStats/
├── SessionStats.tsx              # コンポーネント本体
├── SessionStats.css              # スタイル
├── SessionStats.test.tsx         # テスト
├── LatencyBar.tsx                # 遅延バーサブコンポーネント
├── formatBytes.ts                # バイト数フォーマット関数
└── index.ts                      # エクスポート
```

---

## 関連ドキュメント

- [connection-indicator.md](./connection-indicator.md) - 接続状態表示
- [design-tokens.md](../design-tokens.md) - カラー・スペーシング定義
- [ADR-008: Zero Latency Mode](../../adr/ADR-008-zero-latency-mode.md) - 低遅延優先設計
