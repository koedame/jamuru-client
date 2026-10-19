# MixerPanel - ミキシングコンソール

DAWスタイルのミキシングコンソールコンポーネント。
セッション中の音量・パン調整、レベル監視を行う。

---

## 概要

### 目的
- 自分と参加者の音声をリアルタイムで監視・調整
- DAWライクな操作感で音楽制作に慣れたユーザーに親しみやすいUI

### 使用場面
- セッション画面のメインUI
- 音量バランス調整時

---

## ビジュアル仕様

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ ミキサー                                                                    │
├────────┬────────┬────────┬────────┐                    ┌──────────────────┤
│ 音質   │48kHz   │48kHz   │44.1kHz │                    │     -23db -23db  │
│        │ /2ch   │ /1ch   │ /2ch   │                    │                  │
├────────┼────────┼────────┼────────┤                    │   ┌───┐ ┌───┐   │
│ PAN    │ ├──┼──┤│ ├──┼──┤│ ├──┼──┤│                    │   │   │ │   │   │
├────────┼────────┼────────┼────────┤                    │   │   │ │   │   │
│        │-0.0    │-0.0    │-0.0    │                    │   │   │ │   │   │
│        │-23db   │-23db   │-23db   │                    │   │   │ │   │   │
├────────┼────────┼────────┼────────┤                    │   │   │ │   │   │
│        │ ██ ██  │ ██ ██  │ ██ ██  │                    │   │   │ │   │   │
│ 音量   │ ██ ██  │ ██ ██  │ ██ ██  │                    │   └───┘ └───┘   │
│        │ ░░ ░░  │ ░░ ░░  │ ░░ ░░  │                    │      L     R     │
│        │ ░░ ░░  │ ░░ ░░  │ ░░ ░░  │                    │                  │
│        │ [─] [─]│ [─] [─]│ [─] [─]│                    │    マスター      │
├────────┼────────┼────────┼────────┤                    │                  │
│ 名前   │  自分  │  XXXX  │  XXXX  │                    │  ┌────────────┐  │
├────────┼────────┼────────┼────────┤                    │  │   退室     │  │
│ミュート│   🎤   │   🔇   │   🔇   │                    │  └────────────┘  │
└────────┴────────┴────────┴────────┘                    └──────────────────┘
```

> **注（ui.pen Screens/Main）**: 上図右側のマスターメーターと退室ボタンは、現在の実装では
> MixerPanel ではなくルームサイドバー（`MainScreen.tsx`、幅 240px）に配置する。MixerPanel は
> チャンネルストリップのみを保持し、`local`（自分）を入力セクション、`remote`（参加者）を
> 出力セクションとして縦の区切り線（`--color-border`）で分割して横に並べる。マスターは
> 水平メーター（`MasterSection` = `Molecules/Meter/MasterHorizontal`）としてサイドバーに表示する。
> ソロ機能は ui.pen に存在しないため持たない。

---

## コンポーネント構成

### MixerPanel（コンテナ）

全体を管理するルートコンポーネント。

```typescript
interface MixerPanelProps {
  /** 表示するチャンネル配列（ローカル + リモート） */
  channels: Channel[];
  /** 音量変更コールバック */
  onChannelVolumeChange?: (channelId: string, volume: number) => void;
  /** パン変更コールバック */
  onChannelPanChange?: (channelId: string, pan: number) => void;
  /** ミュート切替コールバック */
  onChannelMuteToggle?: (channelId: string) => void;
  /** モニタリング切替コールバック（ローカルチャンネルのみ） */
  onChannelMonitorToggle?: (channelId: string) => void;
}

interface Channel {
  id: string;
  name: string;
  type: "local" | "remote";
  sampleRate: number;      // e.g., 48000
  channelCount: number;    // 1 (mono) or 2 (stereo)
  levelL: number;          // 0-100 (left channel level)
  levelR: number;          // 0-100 (right channel level)
  volume: number;          // 0-100
  pan: number;             // -100 to 100 (L to R)
  isMuted: boolean;
  isMonitoring?: boolean;  // ローカルチャンネルのみ。自分の音を直接聞いているか
}
```

`MixerPanel` は `channels` を `type` で分類し、`local`（自分）を入力セクション、
`remote`（参加者）を出力セクションとして縦の区切り線で分けて横に並べる。マスター出力・
退室ボタンは MixerPanel に含めず、接続済み画面のルームサイドバー（`MainScreen.tsx`）に
配置する。

---

### ChannelStrip（チャンネルストリップ）

個別のチャンネルを表示・操作するコンポーネント。

```typescript
interface ChannelStripProps {
  /** チャンネルID */
  id: string;
  /** 表示名 */
  name: string;
  /** チャンネルタイプ */
  type: "local" | "remote";
  /** サンプリングレート (Hz) */
  sampleRate: number;
  /** チャンネル数 (1 or 2) */
  channelCount: number;
  /** 左チャンネルレベル (0-100) */
  levelL: number;
  /** 右チャンネルレベル (0-100) */
  levelR: number;
  /** 音量 (0-100) */
  volume: number;
  /** パン (-100 to 100) */
  pan: number;
  /** ミュート状態 */
  isMuted: boolean;
  /** 音量変更コールバック */
  onVolumeChange?: (volume: number) => void;
  /** パン変更コールバック */
  onPanChange?: (pan: number) => void;
  /** ミュート切替コールバック */
  onMuteToggle?: () => void;
  /** 自分の音を直接聞いているか（ローカルチャンネルのみ） */
  isMonitoring?: boolean;
  /** モニタリング切替コールバック。ローカルチャンネルで、渡されたときだけボタンを出す */
  onMonitorToggle?: () => void;
}
```

#### 構成要素（上から下）

1. **AudioQualityBadge** - 音質表示
   - 形式: `{sampleRate/1000}kHz/{channels}ch`
   - 例: "48kHz/2ch", "44.1kHz/1ch"

2. **PanSlider** - パンスライダー
   - 水平方向のスライダー
   - 範囲: -100（左）〜 0（中央）〜 100（右）
   - ダブルクリックで中央にリセット

3. **値表示** - フェーダー値・ピーク値
   - フェーダー値（例: "-0.0"）をフェーダー上部、ピーク dB 値（例: "-23db"）をメーター上部に表示
   - フェーダー（左）とメーター（右）を横並びで配置

4. **StereoMeter** - ステレオレベルメーター
   - L/R 2本のメーター
   - 色分け:
     - -∞ to -12dB: 緑 (`--color-meter-low`)
     - -12 to -3dB: 黄 (`--color-meter-mid`)
     - -3 to 0dB: 赤 (`--color-meter-high`)

5. **StereoFader** - ステレオ音量フェーダー
   - L/R 2本のフェーダー（連動）
   - 範囲: 0-100
   - ダブルクリックで80（0dB）にリセット

6. **ChannelLabel** - 名前ラベル
   - ローカル: "自分" (i18n: `mixer.self`)
   - リモート: ユーザー名

7. **MuteButton** - ミュートボタン（全幅・下部）
   - ローカル: マイクアイコン（lucide `mic` / ミュート時 `mic-off`）
   - リモート: スピーカーアイコン（lucide `volume-2` / ミュート時 `volume-x`）
   - アクティブ時アイコン `--color-accent`、ミュート時 `--color-text-secondary`

8. **MonitorButton** - モニタリングボタン（ローカルのみ。ミュートボタンの右に並べる）
   - ヘッドホンアイコン（lucide `headphones`）。ON のときアイコンと枠が `--color-accent`、OFF は `--color-text-secondary`
   - 自分の音をネットワーク遅延なしで聞く（[ADR-033](../../adr/ADR-033-local-monitoring.md)）。既定は OFF で、セッションの開始ごとに OFF へ戻る
   - ミュートとは独立。ミュート中でも自分の音は聞こえる
   - `aria-pressed` で状態を伝える。ラベルは `mixer.channel.monitor`、ツールチップは `mixer.channel.monitorHint`

---

### MasterSection（マスターセクション）

```typescript
interface MasterSectionProps {
  /** 左チャンネルレベル (0-100) */
  levelL: number;
  /** 右チャンネルレベル (0-100) */
  levelR: number;
  /** ミュート状態（メーターをグレーアウト） */
  isMuted?: boolean;
}
```

ui.pen の `Molecules/Meter/MasterHorizontal` に準拠した**水平表示専用**コンポーネント。
接続済み画面では MixerPanel 内ではなくルームサイドバーに配置する。退室ボタン・ミュート
ボタンは持たない（退室はサイドバーの退出ボタン、ミュートは各チャンネル側）。

#### 構成要素
1. **Header** - タイトル「マスター」(i18n: `mixer.master`) + ピーク dB 値（`usePeakHold` 由来）
2. **MetersArea** - L/R の水平レベルメーター（左→右で緑→黄→赤のグラデーション）+ 0dB ライン

---

## サブコンポーネント

### StereoMeter

```typescript
interface StereoMeterProps {
  /** 左チャンネルレベル (0-100) */
  levelL: number;
  /** 右チャンネルレベル (0-100) */
  levelR: number;
  /** 高さ (px) */
  height?: number;
  /** ミュート状態（メーターをグレーアウト） */
  isMuted?: boolean;
}
```

### StereoFader

```typescript
interface StereoFaderProps {
  /** 音量 (0-100) */
  volume: number;
  /** 高さ (px) */
  height?: number;
  /** 無効状態 */
  disabled?: boolean;
  /** 変更コールバック */
  onChange?: (volume: number) => void;
}
```

### PanSlider

```typescript
interface PanSliderProps {
  /** パン値 (-100 to 100) */
  value: number;
  /** 無効状態 */
  disabled?: boolean;
  /** 変更コールバック */
  onChange?: (value: number) => void;
}
```

### AudioQualityBadge

```typescript
interface AudioQualityBadgeProps {
  /** サンプリングレート (Hz) */
  sampleRate: number;
  /** チャンネル数 */
  channels: number;
}
```

---

## サイズ仕様

| 要素 | サイズ |
|------|--------|
| チャンネルストリップ幅 | 100px |
| マスターセクション幅 | 固定値なし。ルームサイドバー（240px）の内側に収まる可変幅の水平メーター |
| チャンネルストリップ高さ | ミキサーの縦いっぱいに伸びる（セクションと同じ高さ） |
| メーター高さ | 最小 200px。ストリップの余った高さまで伸びる |
| フェーダー高さ | 最小 200px。ストリップの余った高さまで伸びる |
| パンスライダー幅 | 80px |
| チャンネル間スペース | 8px |

---

## アクセシビリティ

### ARIA属性

```tsx
// StereoFader
<input
  type="range"
  role="slider"
  aria-label={t("mixer.volume", "Volume")}
  aria-valuemin={0}
  aria-valuemax={100}
  aria-valuenow={volume}
  aria-valuetext={`${volume}%`}
/>

// PanSlider
<input
  type="range"
  role="slider"
  aria-label={t("mixer.pan", "Pan")}
  aria-valuemin={-100}
  aria-valuemax={100}
  aria-valuenow={pan}
  aria-valuetext={panToText(pan)} // "Left 50%", "Center", "Right 30%"
/>

// MuteButton
<button
  aria-label={t(isMuted ? "mixer.unmute" : "mixer.mute")}
  aria-pressed={isMuted}
/>
```

退室ボタンは MixerPanel の外（ルームサイドバー）にあるため、そちらのコンポーネント
仕様（`docs-spec/ui/multi-window-architecture.md` の接続後レイアウト）を参照。

### キーボード操作

| 要素 | キー | 動作 |
|------|------|------|
| フェーダー | ↑/↓ | 1ずつ増減 |
| フェーダー | Page Up/Down | 10ずつ増減 |
| パンスライダー | ←/→ | 5ずつ増減 |
| パンスライダー | Home/End | 最小/最大 |
| ミュートボタン | Space/Enter | トグル |
| モニタリングボタン | Space/Enter | トグル |

退室ボタン（Space/Enter で確認ダイアログ表示）は MixerPanel の外（ルームサイドバー）。

---

## i18n キー

```json
{
  "mixer.self": "自分",
  "mixer.master": "マスター",
  "mixer.volume": "音量",
  "mixer.pan": "パン",
  "mixer.mute": "ミュート",
  "mixer.unmute": "ミュート解除",
  "mixer.channel.monitor": "モニタリング",
  "mixer.channel.monitorHint": "自分の音を遅延なしで聞く",
  "mixer.leave": "退室",
  "mixer.leaveConfirm": "セッションから退室しますか？",
  "mixer.audioQuality": "{{rate}}kHz/{{ch}}ch"
}
```

---

## 使用例

`channels` は `local`/`remote` を混在させた単一配列で渡す（`MixerPanel` 内部で
`type` により分類する）。マスター出力・退室は含まない（ルームサイドバー側、上記参照）。

```tsx
<MixerPanel
  channels={[
    {
      id: "local",
      name: "自分",
      type: "local",
      sampleRate: 48000,
      channelCount: 2,
      levelL: 65,
      levelR: 70,
      volume: 80,
      pan: 0,
      isMuted: false,
    },
    {
      id: "peer-1",
      name: "山田太郎",
      type: "remote",
      sampleRate: 48000,
      channelCount: 1,
      levelL: 45,
      levelR: 45,
      volume: 75,
      pan: -20,
      isMuted: false,
    },
  ]}
  onChannelVolumeChange={handleChannelVolumeChange}
  onChannelPanChange={handleChannelPanChange}
  onChannelMuteToggle={handleChannelMuteToggle}
/>
```

---

---

## サブコンポーネント詳細

### StereoMeter（ステレオレベルメーター）

Canvas ベースのリアルタイムレベル表示。

```typescript
interface StereoMeterProps {
  /** 左チャンネルレベル (0-100) */
  levelL: number;
  /** 右チャンネルレベル (0-100) */
  levelR: number;
  /** 高さ (px)。省略するとスタイルで決まる（既定 200px、フレックスの親の中では親に合わせて伸びる） */
  height?: number;
  /** ミュート状態 */
  isMuted?: boolean;
  /** ピークリリース時間 (ms) - default: 1500 */
  peakReleaseTime?: number;
}
```

#### 描画仕様

- L/R 2本のバー + dBスケール
- レベル 0-100 を擬似対数スケールで表示
- ピークホールド: 500ms 保持 → releaseTime で減衰

#### 色分け（dB → 表示色）

| dBレベル | 色 | 位置 (%) |
|---------|-----|---------|
| 0 dB | 赤 (`--color-meter-high`) | 100% |
| -6 dB | 黄 | 85% |
| -12 dB | 黄 (`--color-meter-mid`) | 70% |
| -18 dB | 緑 | 55% |
| -24 dB | 緑 | 40% |
| -30 dB | 緑 (`--color-meter-low`) | 27% |
| -40 dB | 緑 | 14% |
| -50 dB | 緑 | 3% |

#### dB 変換式

```typescript
function levelToDb(level: number): string {
  if (level <= 0) return "-∞";
  const db = (level / 100) * 60 - 60; // 0-100 → -60dB〜0dB
  if (db >= 0) return "0.0";
  return db.toFixed(1);
}
```

---

### StereoFader（ステレオ音量フェーダー）

縦型のボリュームコントロール。

```typescript
interface StereoFaderProps {
  /** 音量 (0-100) */
  volume: number;
  /** 高さ (px)。省略するとスタイルで決まる（既定 200px、フレックスの親の中では親に合わせて伸びる） */
  height?: number;
  /** 無効状態 */
  disabled?: boolean;
  /** 変更コールバック */
  onChange?: (volume: number) => void;
  /** アクセシビリティラベル - default: "Volume" */
  label?: string;
}
```

#### スケール目盛り

| 位置 (%) | 意味 |
|---------|------|
| 80% | 0dB (ユニティゲイン) - 強調表示 |
| 60% | - |
| 40% | - |
| 20% | - |

#### 操作

| 操作 | 動作 |
|------|------|
| ドラッグ | 音量変更 |
| ↑/↓ | 1ずつ増減 |
| Page Up/Down | 10ずつ増減 |
| Home | 最大 (100) |
| End | 最小 (0) |
| ダブルクリック | 0dB (80) にリセット |

---

### PanSlider（パンスライダー）

水平方向のパンコントロール。

```typescript
interface PanSliderProps {
  /** パン値 (-100: 左, 0: 中央, 100: 右) */
  value: number;
  /** 無効状態 */
  disabled?: boolean;
  /** 変更コールバック */
  onChange?: (value: number) => void;
  /** アクセシビリティラベル - default: "Pan" */
  label?: string;
}
```

#### 操作

| 操作 | 動作 |
|------|------|
| ドラッグ | パン変更 |
| ←/→ | 値を変更 |
| ダブルクリック | 中央 (0) にリセット |

#### ARIA属性

```tsx
<input
  type="range"
  aria-label={label}
  aria-valuemin={-100}
  aria-valuemax={100}
  aria-valuenow={value}
/>
```

---

### AudioQualityBadge（音質バッジ）

サンプリングレートとチャンネル数を表示。

```typescript
interface AudioQualityBadgeProps {
  /** サンプリングレート (Hz) */
  sampleRate: number;
  /** チャンネル数 (1 or 2) */
  channels: number;
}
```

#### 表示形式

```
{sampleRate/1000}kHz/{channels}ch
```

例: `48kHz/2ch`, `44.1kHz/1ch`

---

### usePeakHold（フック）

ピークレベルのホールド・減衰を管理するカスタムフック。

```typescript
interface PeakHoldOptions {
  /** ホールド時間 (ms) - default: 500 */
  holdTime?: number;
  /** 減衰時間 (ms) - default: 1500 */
  releaseTime?: number;
}

interface PeakHoldState {
  peakL: number;
  peakR: number;
}

function usePeakHold(
  levelL: number,
  levelR: number,
  options?: PeakHoldOptions
): PeakHoldState;
```

#### 動作

1. 新しいピーク検出 → 即座に更新
2. `holdTime` (500ms) 経過後 → 減衰開始
3. `releaseTime` (1500ms) かけて 0 まで減衰
4. 減衰中に新しいピーク検出 → 即座にピーク更新

#### 使用例

```tsx
const { peakL, peakR } = usePeakHold(levelL, levelR, {
  holdTime: 600,
  releaseTime: 200,
});
```

---

## 関連ドキュメント

- [design-tokens.md](../design-tokens.md) - カラー・スペーシング定義
- [README.md](./README.md) - コンポーネントカタログ
