---
sidebar_label: Audio Engine
sidebar_position: 1
---

<!-- このドキュメントは実装の正です。変更時は実装も同期すること -->

# Audio Engine API

音声キャプチャ・再生エンジンの内部API定義。

---

## 1. 概要

Audio Engineは以下の責務を持つ:

- オーディオデバイスの列挙・選択
- 音声キャプチャ（入力）
- 音声再生（出力）
- ローカルモニタリング
- サンプルレート変換
- デバイス切断時の自動フォールバック

### 1.1 自動開始仕様

アプリケーション起動時、オーディオエンジンは**自動的に開始**される:

- **開始タイミング**: Tauriアプリケーションの`setup`フック内
- **使用デバイス**: システムのデフォルト入力/出力デバイス
- **Start/Stopボタン**: UIに残すが、デバイス切り替えは即時反映
- **失敗時**: エラーログを出力し、ユーザーは手動でデバイスを選択して再試行

### 1.2 デバイス即時切り替え

デバイス選択UIでデバイスを変更した場合:

- **入力デバイス**: 即座にキャプチャストリームを再作成（10-50msの音切れ）
- **出力デバイス**: 即座に再生ストリームを再作成（10-50msの音切れ）。再生バッファは作り直さず同じものを新しいストリームに渡すので、バッファ内の音声は切替後に残る
- 停止→再開始の操作は不要

### 1.3 デバイス切断時の自動フォールバック

使用中のデバイスが切断された場合:

- cpalの`StreamError::DeviceNotAvailable`を検知
- システムのデフォルトデバイスに自動切り替え
- フロントエンドに`device-disconnected`イベントを送信
- UIにトースト通知を表示

---

## 2. モジュール構成

```
audio/
├── codec.rs        # コーデック（PCM, Opus）
├── device.rs       # デバイス管理
├── engine.rs       # オーディオエンジン（キャプチャ・再生）
├── error.rs        # エラー型
├── playout.rs      # 受信音声の再生バッファ（ADR-028）
├── plc.rs          # Packet Loss Concealment
├── preset.rs       # プリセット定義と遅延バジェット（ADR-019）
├── resampler.rs    # サンプルレート変換（ADR-013）
└── stream.rs       # 送受信の音声形式と受信経路（§5.4）
```

---

## 3. デバイス管理 API

### 3.1 デバイス列挙

```rust
/// 利用可能な入力デバイス一覧を取得
///
/// スレッド: 任意
/// ブロッキング: No
fn list_input_devices() -> Result<Vec<AudioDevice>, AudioError>;

/// 利用可能な出力デバイス一覧を取得
///
/// スレッド: 任意
/// ブロッキング: No
fn list_output_devices() -> Result<Vec<AudioDevice>, AudioError>;
```

### 3.2 デバイス情報

```rust
struct AudioDevice {
    /// デバイス識別子
    id: DeviceId,
    /// 表示名
    name: String,
    /// 対応サンプルレート
    supported_sample_rates: Vec<u32>,
    /// 対応チャンネル数
    supported_channels: Vec<u16>,
    /// デフォルトデバイスかどうか
    is_default: bool,
    /// ASIO対応（Windowsのみ）
    is_asio: bool,
}
```

### 3.3 デバイス選択

```rust
/// 入力デバイスを設定
///
/// スレッド: 非リアルタイムスレッドから呼び出すこと
/// ブロッキング: Yes（デバイスオープン完了まで）
///
/// # 注意
/// キャプチャ中に呼び出した場合、キャプチャは停止される
fn set_input_device(device_id: DeviceId) -> Result<(), AudioError>;

```

出力デバイスを設定する `AudioEngine` の API は無い。再生ストリームは供給元（§5.2 の `fill_frame`）を所有して動くので、エンジン単独では作り直せない。呼び出し側が `stop_playback` してから、同じ受信経路を指す供給元で `start_playback_with_source` を呼び直す（GUI の `SetOutputDevice` コマンドがこの形）。

---

## 4. キャプチャ API

### 4.1 設定

```rust
struct CaptureConfig {
    /// サンプルレート（Hz）
    sample_rate: u32,
    /// チャンネル数
    channels: u16,
    /// フレームサイズ（サンプル数）
    frame_size: u32,
    /// ビット深度
    bit_depth: BitDepth,
}

enum BitDepth {
    I16,
    I24,
    F32,
}
```

### 4.2 キャプチャ開始・停止

```rust
/// 音声キャプチャを開始
///
/// スレッド: 非リアルタイムスレッドから呼び出すこと
/// ブロッキング: No（即座にリターン、キャプチャは別スレッドで実行）
///
/// # コールバック
/// キャプチャされた音声データは `on_audio_captured` コールバックで通知される
fn start_capture(config: CaptureConfig) -> Result<(), AudioError>;

/// 音声キャプチャを停止
///
/// スレッド: 非リアルタイムスレッドから呼び出すこと
/// ブロッキング: Yes（キャプチャスレッド終了まで）
fn stop_capture() -> Result<(), AudioError>;
```

### 4.3 コールバック

```rust
/// 音声キャプチャコールバック
///
/// スレッド: リアルタイムオーディオスレッドから呼び出される
///
/// # 制約
/// - メモリアロケーション禁止
/// - ブロッキングI/O禁止
/// - ミューテックスの長時間保持禁止
/// - 処理時間: frame_size / sample_rate 以内（例: 128/48000 = 2.67ms）
fn on_audio_captured(data: &AudioBuffer, timestamp: u64);
```

---

## 5. 再生 API

### 5.1 設定

```rust
struct PlaybackConfig {
    /// サンプルレート（Hz）
    sample_rate: u32,
    /// チャンネル数
    channels: u16,
    /// フレームサイズ（サンプル数）
    frame_size: u32,
    /// ビット深度
    bit_depth: BitDepth,
}
```

### 5.2 再生開始・停止

```rust
/// 音声再生を開始する。出力コールバックが、デバイスのクロックで供給元から 1 フレームずつ引く（ADR-028）
///
/// スレッド: 非リアルタイムスレッドから呼び出すこと
/// ブロッキング: No
///
/// # 引数
/// - device_id: None はシステムのデフォルト出力
/// - frame_samples: 供給元の 1 フレームのインターリーブサンプル数。これの 3 倍を受け渡しバッファに確保する
/// - fill_frame: 受け渡しバッファへ書き、書いたサンプル数を返す。0 は「無い」で、その分は無音になる。
///   リアルタイムスレッドで呼ばれるので、アロケートしない・待たない
fn start_playback_with_source<F>(device_id: Option<&DeviceId>, frame_samples: usize, fill_frame: F)
    -> Result<(), AudioError>
    where F: FnMut(&mut [f32]) -> usize + Send + 'static;

/// 音声再生を停止
///
/// スレッド: 非リアルタイムスレッドから呼び出すこと
/// ブロッキング: Yes
fn stop_playback();
```

### 5.3 音声データの引き取り

再生側に、音声データを積む API は無い。**供給元が再生時刻まで保持し、出力コールバックが要求のたびに 1 フレーム引く**。後段のキューが無いので、供給元が保持している遅延が聞こえる遅延になる（[ADR-028](../adr/ADR-028-single-stage-playout.md)）。旧来の再生リングバッファ（`start_playback` / `enqueue_playback` / `playback_vacant_samples`）は廃止した。

出力コールバックの規則（REQ-AUD-031）:

- 直前に引いたフレームをデバイスが使い切ったときにだけ、供給元へ次のフレームを求める。先読みしない
- デバイスの 1 回の要求量はフレーム長と一致しない。フレームの余りは次の要求へ繰り越し、欠落・重複させない
- 供給元が 0 を返したら、その要求の残りを無音にする。前のフレームの残りを鳴らさない
- フレーム長は可変（相手が別のサンプルレートだと変換後の長さが揃わない）。`fill_frame` の戻り値をそのまま使い、固定長に丸めない（丸めるとピッチが変わる）

受信音声の供給元は §5.4 の `ReceivePath::read_into` である。

### 5.4 受信経路（`ReceivePath`）

受信した音声がソケットから出力コールバックに届くまでの経路。CLI と GUI が同じものを使い、両者の音の出方を一致させる（ADR-027, ADR-028）。

```rust
/// 送受信するチャンネル数。送信側がパンを適用したステレオを送る
pub const WIRE_CHANNELS: usize = 2;

/// キャプチャしたモノラルフレームを送信用ステレオに変換する
/// volume: 1.0 = 等倍 / pan: -100（左）〜 100（右）、等パワー則
pub fn mono_to_wire(mono: &[f32], volume: f32, pan: i32, out: &mut [f32]);

/// キャプチャしたフレーム（channels = 1 または 2、インターリーブ）を送信用ステレオに変換する
/// モノラルは `mono_to_wire` と同じ（パンで定位を決める）。ステレオは左右を混ぜず、
/// pan はバランスとして働く（向いた側は変えず、反対側を絞る。中央は素通し）
pub fn capture_to_wire(captured: &[f32], channels: usize, volume: f32, pan: i32, out: &mut [f32]);

/// 受信経路。Clone はすべて同じバッファを共有する
#[derive(Clone)]
pub struct ReceivePath { /* ... */ }

impl ReceivePath {
    /// バッファの設定は PlayoutConfig::for_delay(frame_samples, target_delay_frames)
    pub fn new(codec_type: CodecType, sample_rate: u32, frame_size: u32,
               target_delay_frames: u32) -> Result<Self, CodecError>;
    /// 1 フレームのインターリーブサンプル数（出力デバイスを開く単位）
    pub fn frame_samples(&self) -> usize;
    /// ネットワーク側: デコード → （必要なら）レート変換 → プレイアウトバッファ
    pub fn receive(&self, sequence: u32, payload: &[u8]) -> bool;
    /// 出力コールバック側: 次のフレームを書き込む。待たない
    /// （ネットワーク側がロック中なら 1 フレームぶんの無音を Priming として返す）
    /// 結果の意味は §8.3 の PlayoutResult と同じ
    pub fn read_into(&self, out: &mut [f32]) -> PlayoutRead;
    /// 接続（再接続）時に保持中のフレームを捨てる（ADR-022）
    pub fn reset(&self);
    /// プリセットの切替で遅延段数を選び直す。保持量が新しい段数へ動く（ADR-031）。
    /// 範囲（最小・最大）も切替先の段数から決まり、接続時の範囲では切られない
    pub fn set_delay_frames(&self, frames: u32);
    /// バッファが実際に保っている段数。自動調整・再接続でも動くので、呼び出し側で覚えず読む
    pub fn delay_frames(&self) -> u32;
    /// 直近の区間の補間率に応じて段数を上下させる。動いたら新しい段数を返す。1 秒ごとに呼ぶ（ADR-031）
    pub fn adapt(&self) -> Option<u32>;
    /// 相手のサンプルレートに追従する（ADR-013）
    pub fn follow_peer_rate(&self, peer_rate: u32) -> PeerRateChange;
    pub fn stats(&self) -> PlayoutStats;
}

pub enum PeerRateChange {
    Unchanged,
    Resampling { from: u32, to: u32, latency_ms: f32 },
    Passthrough,
    Failed(ResamplerError),
}
```

音量・パンなどミキサーの処理は GUI だけが `read_into` の結果に掛ける（CLI にミキサーは無い）。

---

## 6. ローカルモニタリング API

自分の入力を、ネットワークもジッタバッファも通さずに出力へ混ぜる（[ADR-033](../adr/ADR-033-local-monitoring.md)）。

```rust
/// キャプチャと出力の間に立つモニタ。Clone で共有できる
pub struct LocalMonitor { /* ... */ }

impl LocalMonitor {
    /// キャプチャが届けるフレーム長（モノラルのサンプル数）を渡す。OFF・音量 1.0 で始まる
    pub fn new(frame_size: u32) -> Self;

    /// ローカルモニタリングを有効化・無効化する
    ///
    /// スレッド: 任意
    /// ブロッキング: No
    ///
    /// 無効の間は何も溜めない。無効にした時点で溜まっていた音は、有効に戻しても鳴らない
    pub fn set_enabled(&self, enabled: bool);
    pub fn is_enabled(&self) -> bool;

    /// ローカルモニタリングの音量を設定する。範囲外は丸め、NaN は無視する
    ///
    /// # 引数
    /// - volume: 0.0（無音）〜 1.0（キャプチャしたまま）
    pub fn set_volume(&self, volume: f32);
    pub fn volume(&self) -> f32;

    /// キャプチャコールバックへ渡す端。呼ぶたびに新しいリングを作り、前の端は使えなくなる
    /// （入力デバイスの切り替えで作り直す）
    pub fn tap(&self) -> MonitorTap;

    /// 出力コールバックから呼ぶ。`out`（WIRE_CHANNELS 交互配置）へ入力を加算する
    ///
    /// スレッド: リアルタイム（出力コールバック）
    /// ブロッキング: No（アロケートしない。キャプチャ側がリングを差し替えている間は、
    /// そのフレームだけモニタリングなしで返る）
    pub fn mix_into(&self, out: &mut [f32]);
}

impl MonitorTap {
    /// キャプチャしたモノラルのフレームを渡す
    ///
    /// スレッド: リアルタイム（キャプチャコールバック）
    /// ブロッキング: No
    pub fn push(&mut self, mono: &[f32]);

    /// チャンネル数 `channels` のインターリーブされたフレームを渡す。
    /// 2 チャンネル以上は左右の平均（モニターはモノラル。ADR-033）
    ///
    /// スレッド: リアルタイム（キャプチャコールバック）
    /// ブロッキング: No
    pub fn push_interleaved(&mut self, samples: &[f32], channels: usize);
}
```

- 遅延はキャプチャ 1 フレーム + 余裕 `MONITOR_MARGIN_FRAMES`（1）フレーム + 再生 1 フレーム
- モノラルの入力を両チャンネルへ同じ大きさで足す。パンは掛けない
- 出力に既にある相手の音声へ加算する。相手の音声が無い間（再生バッファの起動中・枯渇）も聞こえる
- ミュート（相手へ送らない）とは独立

---

## 7. ミキサー API

```rust
/// 参加者の音量を設定
///
/// スレッド: 任意（アトミック操作）
/// ブロッキング: No
///
/// # 引数
/// - participant_id: 参加者ID
/// - volume: 0.0（無音）〜 1.0（最大）
fn set_participant_volume(participant_id: ParticipantId, volume: f32);

/// 参加者をミュート
fn mute_participant(participant_id: ParticipantId);

/// 参加者のミュートを解除
fn unmute_participant(participant_id: ParticipantId);

/// マスター音量を設定
fn set_master_volume(volume: f32);
```

---

## 8. バッファ

### 8.1 AudioBuffer

```rust
struct AudioBuffer {
    /// サンプルデータ（インターリーブ形式）
    data: Vec<f32>,
    /// チャンネル数
    channels: u16,
    /// サンプル数（チャンネルあたり）
    samples: u32,
}
```

### 8.2 リングバッファ

キャプチャコールバックから送信スレッドへ音声を渡す経路にだけ使う（`rtrb` の SPSC）。再生側には無い。受信した音声は §8.3 の再生バッファが保持し、出力コールバックが直接読む（ADR-028）。

```rust
/// ロックフリーリングバッファ
///
/// 単一プロデューサー・単一コンシューマー（SPSC）
struct RingBuffer<T> {
    // ...
}

impl<T> RingBuffer<T> {
    /// バッファを作成
    fn new(capacity: usize) -> Self;

    /// データをプッシュ（プロデューサー側）
    /// ブロッキング: No
    fn push(&self, item: T) -> Result<(), T>;

    /// データをポップ（コンシューマー側）
    /// ブロッキング: No
    fn pop(&self) -> Option<T>;

    /// 現在のアイテム数
    fn len(&self) -> usize;
}
```

### 8.3 再生バッファ（PlayoutBuffer）

受信した音声が再生を待つ唯一の場所。出力コールバックがここから直接読む（[ADR-028](../adr/ADR-028-single-stage-playout.md)）。ジッタバッファ・PLC を兼ねる。

```rust
pub struct PlayoutConfig {
    /// 1フレームのサンプル数（全チャンネル分、インターリーブ）
    pub frame_samples: usize,
    /// 再生より先に保持するフレーム数（段数）。0 はパススルー
    pub target_delay_frames: u32,
    /// 適応で選べる段数の下限
    pub min_delay_frames: u32,
    /// 適応で選べる段数の上限。スロット数の算出にも使う
    pub max_delay_frames: u32,
}

impl PlayoutConfig {
    /// 受信経路が使う設定。min = 段数が 0 なら 0、それ以外は 1。max = max(段数, 1) × 2 + 2
    pub fn for_delay(frame_samples: usize, target_delay_frames: u32) -> Self;
}

pub enum PlayoutResult {
    /// 再生すべきフレームが届いていた
    Played { sequence: u32 },
    /// 届いておらず、後続フレームが届いているので損失と判定して補間した
    Concealed { sequence: u32 },
    /// 再生開始に必要な量がまだ溜まっていない。出力は無音
    Priming,
    /// 段数を増やすために無音を出している。再生位置は進めない（ADR-031）。
    /// アンダーランでも損失でもないので、`frames_concealed` には入らない
    Padded,
    /// 再生中だが、再生すべきフレームも後続も届いていない。出力は無音で、再生位置は進めない
    Starved,
}

impl PlayoutBuffer {
    pub fn new(config: PlayoutConfig) -> Self;

    /// 最大 `ring_delay_frames` の段数まで `set_delay` で動かせるリングを確保して作る。
    /// 再生中に確保し直さないため、全プリセットの最大段数で作る（ADR-031）
    pub fn with_ring_for(config: PlayoutConfig, ring_delay_frames: u32) -> Self;

    /// デコード済みフレームを格納する（ネットワーク側から呼ぶ）
    pub fn write(&mut self, sequence: u32, samples: &[f32]) -> WriteOutcome;

    /// 次のフレームを `out` に書く（出力コールバックから呼ぶ）
    /// アロケートせず、ブロックしない
    pub fn read_into(&mut self, out: &mut [f32]) -> PlayoutRead;

    /// 保持しているフレーム数
    pub fn ready_frames(&self) -> usize;

    /// 段数の取得・変更（変更は min〜max にクランプ。自動調整の経路）。
    /// 再生中の変更は保持量に効く（下記「再生中の段数変更」）
    pub fn target_delay_frames(&self) -> u32;
    pub fn set_target_delay_frames(&mut self, frames: u32);

    /// 利用者がプリセットを選んだ。段数と、自動調整が動かせる範囲を選んだ段数から決め直す。
    /// `reset` が戻る先もこの段数になる。上限はリングの大きさだけ（ADR-031）
    pub fn set_delay(&mut self, frames: u32);

    /// 直近の区間（前回の判定以降）の補間率に応じて段数を上下させる。
    /// 動いたら新しい段数を返す。1 秒ごとに呼ぶ
    pub fn adapt(&mut self) -> Option<u32>;

    pub fn stats(&self) -> PlayoutStats;
    pub fn reset(&mut self);
}
```

**段数の決まり方:**

| 設定 | 動作 | 要求 |
|------|------|------|
| `target_delay_frames = 0` | パススルー。1フレーム目を到着次第再生し、何も保持しない | REQ-LAT-026 / REQ-LAT-111 |
| `min_delay_frames == max_delay_frames` | 固定。`adapt()` で段数が動かない。`PlayoutConfig::for_delay(0)` は固定（ゼロ遅延の約束を自動調整で崩さない） | REQ-LAT-110 |
| `min_delay_frames < max_delay_frames` | 適応。直近の区間の補間率が 5% 超で段数を 1 増やし、1% 未満の区間が 10 回続いたら 1 減らす。区間の標本が 20 フレーム未満なら判定を見送る | REQ-LAT-108 / REQ-LAT-109 |

**再生中の段数変更（ADR-031）:** 再生が始まった後で段数を変えると、保持量が新しい段数へ動く。

| 変更 | 動作 |
|------|------|
| 増やす | 増やす段数ぶんの読み出しで無音（`Padded`）を出し、再生位置を進めない。その間に届いたフレームが保持量になる |
| 減らす | 保持量が目標を超えている分だけ、再生位置のフレームから捨てる。すでに目標以下ならそれ以上は捨てない |

再生開始前は起動条件が段数を直接見るので、何も起きない。変更が重なったら差分を足し合わせる。`reset` と再同期は保留中の差分を捨てる。

**読み出しの規則:**

- 段数 + 1 フレーム溜まるまで再生を始めない（REQ-LAT-025）
- 再生すべきフレームが無いとき、後続フレームが届いていれば補間（`Concealed`）、何も届いていなければ位置を進めずに待つ（`Starved`）。未到着を損失と扱うと、出力クロックに合わせて再生位置が送信側より先へ進み続け、以後のフレームがすべて遅着になる（REQ-LAT-029、ADR-026）
- 再生位置を過ぎてから届いたフレームは破棄する（`WriteOutcome::Late`）

---

## 9. エラー

```rust
pub enum AudioError {
    /// デバイスが見つからない
    DeviceNotFound(String),
    /// デバイスオープン失敗
    DeviceOpenFailed(String),
    /// サポートされていない設定
    UnsupportedConfig(String),
    /// ストリームエラー
    StreamError(String),
    /// バッファオーバーフロー
    BufferOverflow,
    /// バッファアンダーラン
    BufferUnderrun,
    /// リサンプリングエラー（ADR-013）
    ResamplerError(String),
}
```

---

## 10. リサンプラー API（ADR-013）

ADR-013で決定された受信側リサンプリング戦略を実装するモジュール。

### 10.1 概要

- 送信側: サンプルレート変換なし（ネイティブレートで送信）
- 受信側: ピアのサンプルレートがローカルと異なる場合のみリサンプリング
- 目的: 低遅延優先、必要な場合のみ変換

### 10.2 AudioResampler トレイト

```rust
/// Trait for audio resampling
pub trait AudioResampler: Send {
    /// Process input samples and return resampled output
    fn process(&mut self, input: &[f32]) -> Result<Vec<f32>, ResamplerError>;

    /// Get the latency introduced by resampling in samples (at output rate)
    fn latency_samples(&self) -> usize;

    /// Get the latency introduced by resampling in milliseconds
    fn latency_ms(&self, output_rate: u32) -> f32;
}
```

### 10.3 リサンプラー実装

```rust
/// Passthrough resampler for same sample rate (no conversion)
pub struct PassthroughResampler;

/// Fast resampler using rubato for real-time audio
pub struct FastResampler {
    input_rate: u32,
    output_rate: u32,
    chunk_size: usize,
}

impl FastResampler {
    /// Create a new fast resampler
    ///
    /// # Arguments
    /// * `input_rate` - Input sample rate in Hz
    /// * `output_rate` - Output sample rate in Hz
    /// * `chunk_size` - Expected input chunk size (frame size)
    pub fn new(input_rate: u32, output_rate: u32, chunk_size: usize)
        -> Result<Self, ResamplerError>;
}
```

### 10.4 ファクトリ関数

```rust
/// Create a resampler for the given sample rates
///
/// Returns a PassthroughResampler if input and output rates are the same,
/// otherwise returns a FastResampler.
pub fn create_resampler(
    input_rate: u32,
    output_rate: u32,
    chunk_size: usize,
) -> Result<Box<dyn AudioResampler>, ResamplerError>;

/// インターリーブされた `channels` チャンネル音声用のリサンプラーを作成する
///
/// `chunk_size` はフレーム数（サンプル数ではない）。受信経路はステレオのため、
/// ここに 1 を渡すと 1 サンプルおきにしか変換されず音程がずれる。
pub fn create_resampler_with_channels(
    input_rate: u32,
    output_rate: u32,
    chunk_size: usize,
    channels: usize,
) -> Result<Box<dyn AudioResampler>, ResamplerError>;
```

### 10.5 ResamplerError

```rust
pub enum ResamplerError {
    /// Resampler creation failed
    CreationFailed(String),
    /// Resampling failed
    ProcessFailed(String),
    /// Invalid sample rate
    InvalidSampleRate(u32),
}
```

### 10.6 サポートされるサンプルレート

| サンプルレート | 説明 |
|---------------|------|
| 44100 Hz | CD品質 |
| 48000 Hz | 推奨（標準） |
| 96000 Hz | 高品質 |

---

## 11. デバイスイベント

### 11.1 AudioEvent

オーディオストリームで発生するイベント。デバイス切断検知に使用。

```rust
/// Events that can occur during audio streaming
#[derive(Debug, Clone)]
pub enum AudioEvent {
    /// Input device was disconnected
    InputDeviceDisconnected,
    /// Output device was disconnected
    OutputDeviceDisconnected,
    /// Stream error occurred
    StreamError(String),
}
```

### 11.2 イベント送信の設定

```rust
impl AudioEngine {
    /// Set event sender for device change notifications
    ///
    /// Thread: Non-realtime thread
    /// Must be called before starting capture/playback
    pub fn set_event_sender(&mut self, tx: Sender<AudioEvent>);
}
```

### 11.3 DeviceEvent（サービスレイヤー）

AudioServiceからフロントエンドへ送信されるイベント。

```rust
/// Events sent from the audio thread to notify about device changes
#[derive(Debug, Clone)]
pub enum DeviceEvent {
    /// Input device was disconnected and fallback occurred
    InputDeviceDisconnected {
        /// Device name if fallback succeeded, None if failed
        fallback_device: Option<String>,
    },
    /// Output device was disconnected and fallback occurred
    OutputDeviceDisconnected {
        /// Device name if fallback succeeded, None if failed
        fallback_device: Option<String>,
    },
}
```

### 11.4 Tauri イベント

フロントエンドへは`device-disconnected`イベントとしてemitされる:

```json
{
  "type": "input" | "output",
  "fallback": "default" | null
}
```

---

## 12. スレッドモデル

```
┌─────────────────────────────────────────────────────────┐
│                    Main Thread                          │
│  (デバイス設定、開始/停止)                               │
└─────────────────────────────────────────────────────────┘
                 │                       │
                 ▼                       ▼
        ┌─────────────────┐     ┌─────────────────┐
        │ Capture Thread  │     │ Playback Thread │
        │ (リアルタイム)   │     │ (リアルタイム)   │
        └─────────────────┘     └─────────────────┘
                 │                       ▲
                 ▼                       │ 出力コールバックが 1 フレームずつ引く
   Lock-free Ring Buffer          PlayoutBuffer（§8.3）
                 │                       ▲
                 ▼                       │
        ┌─────────────────┐     ┌─────────────────┐
        │  Send Thread    │     │ Network (tokio) │
        └─────────────────┘     └─────────────────┘
```

§6 のローカルモニタリングは専用のスレッドを持たない。キャプチャコールバックが `MonitorTap::push` でリングへ書き、出力コールバックが `mix_into` で読んで加算する（図の Lock-free Ring Buffer とは別の、モニタ専用のリング）。

---

## 13. 使用例

```rust
// デバイス列挙
let inputs = list_input_devices()?;
let outputs = list_output_devices()?;

// デバイス選択
set_input_device(inputs[0].id)?;
set_output_device(outputs[0].id)?;

// キャプチャ設定
let config = CaptureConfig {
    sample_rate: 48000,
    channels: 1,
    frame_size: 128,
    bit_depth: BitDepth::F32,
};

// コールバック設定
set_capture_callback(|data, timestamp| {
    // ネットワーク送信キューに追加
    network.send_audio(data, timestamp);
});

// 開始
start_capture(config)?;
start_playback_with_source(None, receive.frame_samples(), source)?;
monitor.set_enabled(true);

// ... セッション中 ...

// 停止
stop_capture()?;
stop_playback()?;
```

---

## 14. プリセット API（ADR-019）

プリセットのパラメータと遅延バジェットの唯一の正。`src-tauri` の設定層は本モジュールから `AudioPreset` を再エクスポートし、遅延テストと E2E 品質閾値も同じ値を参照する。数値を他の場所に書き写してはならない（`.claude/rules/traceability.md`）。

### 14.1 遅延モデル

アプリ起因の片道遅延は 3 段のバッファリングの和とする。コーデック起因は既定の非圧縮 PCM f32 で 0ms（ADR-003）。ネットワーク RTT はアプリ起因ではないため含めない。

```
app_latency = capture_buffer + jitter_buffer + playback_buffer
            = frame_duration * (1 + jitter_buffer_frames + 1)
```

### 14.2 AudioPreset

```rust
/// Available audio presets
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AudioPreset {
    ZeroLatency,
    UltraLowLatency,
    #[default]
    Balanced,
    HighQuality,
}

impl AudioPreset {
    /// 1フレームのサンプル数（`AppConfig::buffer_size` に書き込まれる値）
    pub fn frame_size(&self) -> u32;

    /// 定常状態のジッタバッファ段数（0 = パススルー）
    pub fn jitter_buffer_frames(&self) -> u32;

    /// このプリセットが許容されるアプリ起因片道遅延の上限（ms）
    pub fn max_app_latency_ms(&self) -> f32;

    /// 1フレームの長さ（ms）
    pub fn frame_duration_ms(&self, sample_rate: u32) -> f32;

    /// ジッタバッファが加える遅延（ms）
    pub fn jitter_buffer_delay_ms(&self, sample_rate: u32) -> f32;

    /// パラメータから算出されるアプリ起因片道遅延（ms）
    pub fn designed_app_latency_ms(&self, sample_rate: u32) -> f32;

    /// 設定ファイル・IPC 境界で使う識別子
    pub fn name(&self) -> &'static str;

    /// 識別子からのパース（未知の値は None）
    pub fn from_name(name: &str) -> Option<Self>;

    /// 全プリセット（遅延の昇順）
    pub fn all() -> Vec<Self>;
}

/// 遅延バジェットを定義するサンプルレート（48kHz、ADR-013）
pub const BUDGET_SAMPLE_RATE: u32 = 48_000;
```

- スレッド制約: すべて純粋関数。任意のスレッドから呼び出し可能。リアルタイムスレッドでも安全（`all()` のみ Vec を確保するため非リアルタイムスレッド限定）

### 14.3 プリセット定義値

`sample_rate` = 48000Hz の場合。バジェットの根拠は [ADR-019](../adr/ADR-019-preset-latency-budget.md)。

| 識別子 | frame_size | jitter_buffer_frames | 設計値 | バジェット |
|--------|-----------|---------------------|--------|-----------|
| `zero-latency` | 32 | 0 | 1.33ms | 2.0ms |
| `ultra-low-latency` | 64 | 1 | 4.00ms | 5.0ms |
| `balanced` | 128 | 4 | 16.00ms | 18.0ms |
| `high-quality` | 256 | 8 | 53.33ms | 56.0ms |

設計値がバジェットを超えないことは `REQ-LAT-020` が検証する。パラメータを変更した場合、バジェットを超えると `cargo test` が失敗する。

### 14.4 受信経路への反映

`jitter_buffer_frames()` は `src-tauri/src/streaming.rs` の受信経路で `PlayoutConfig::for_delay` の段数として使用される（ADR-020 / ADR-028）。`0` の場合はパススルーになる（§8.3）。

| 項目 | 状態 |
|------|------|
| ジッタバッファ段数の反映 | 実装済み（ADR-020） |
| セッション中のプリセット切替 | 段数のみ即時反映（保持量も動く、ADR-031）。フレームサイズは次回接続時（下記） |
| 適応（`adapt()`）の有効化 | 実装済み（ADR-031）。統計ループが 1 秒ごとに呼び、動いたら「バッファサイズを調整しました」を通知する（REQ-LAT-108）。パススルーは固定 |
| プリセットのコーデック / FEC 設定 | 実装済み（ADR-021）。全プリセットが非圧縮PCM |

### 14.5 セッション中のプリセット切替

プリセットの切り替え（`settings_change` の `{"setting": "preset"}`。ADR-043）がセッション実行中に行われた場合、再生バッファの段数（`set_delay_frames`）を更新する。統計ループがバッファの段数を読み、変わっていれば遅延表示を更新して `jitter_buffer_ms` を `LatencyInfoMessage` で相手に送る。自動調整・再接続による段数の変化も同じ経路で届く（ADR-031）。

| 項目 | 反映タイミング | 理由 |
|------|--------------|------|
| ジッタバッファ段数 | 即時 | 受信経路のみに影響する |
| フレームサイズ | 次回接続時 | `AudioEngine` がキャプチャ・再生バッファ長として保持しており、稼働中の変更はデバイス再初期化を要する |

段数を増やすと無音が入り、減らすとフレームが捨てられる（ADR-031。ADR-028 の「目標値を動かすだけ」を上書きする）。
