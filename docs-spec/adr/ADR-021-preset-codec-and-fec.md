# ADR-021: プリセットのコーデックと FEC 設定

## Status

Accepted

## Context

`audio-quality.feature` はプリセットごとにコーデックと FEC 冗長度を規定しているが、`AudioPreset` はフレームサイズとジッタバッファ段数しか持たない。そのため REQ-AUD-114 / 115 / 116 は `@should` に留まり、検証できるのはフレームサイズと段数のみだった（REQ-AUD-022）。

配線にあたり、以下の不整合が判明した。

### 1. FEC のグループサイズが 3 箇所で食い違う

| 定義場所 | 値 |
|---------|-----|
| ADR-006「パラメータ」 | 冗長度 10%（10パケットにつき1）／グループサイズ 5 |
| `src/network/fec.rs` の `FEC_GROUP_SIZE` | 4 |
| `audio-quality.feature` | balanced 10% / high-quality 20% |

XOR ベース FEC ではグループごとに 1 個の FEC パケットを送るため、冗長度 = 1 / グループサイズ である。ADR-006 の「冗長度 10%」と「グループサイズ 5」（= 20%）は同一文書内で矛盾している。

### 2. グループサイズがジッタバッファ段数を超えると FEC が無意味になる

グループ G の FEC パケットは、グループ最後のデータパケットの後に送られる。したがってグループ内で失われたパケットは、最大で `グループサイズ - 1` フレーム遅れて復元される。

ジッタバッファ段数を `D` とすると、再生位置は最新パケットの `D` フレーム前である。復元が再生に間に合う条件は:

```
グループサイズ <= D
```

`audio-quality.feature` の 10%（グループサイズ 10）は balanced の段数 4 を超えるため、復元されたパケットは再生後に届く。これは帯域を消費して何も改善しない。

### 3. Opus はプリセットのフレームサイズで使用できない

libopus が受け付けるフレーム長は 2.5 / 5 / 10 / 20 / 40 / 60 ms のみであり、48kHz では 120 / 240 / 480 / 960 / 1920 / 2880 サンプルに限られる。

| プリセット | フレームサイズ | Opus で有効か |
|-----------|--------------|-------------|
| zero-latency | 32 | ✗ |
| ultra-low-latency | 64 | ✗ |
| balanced | 128 | ✗ |
| high-quality | 256 | ✗ |

いずれも Opus の有効値ではない。実測でも `frame_size: 128` の符号化は `opus_encode_float: invalid argument` で失敗する。

### 4. Opus 実装は一度もコンパイルされていなかった

`opus-codec` feature を有効化するビルドがどこにも存在しなかったため、`OpusCodec` はコンパイルされたことがなかった。有効化すると 2 種類のエラーが出る。

| エラー | 原因 |
|--------|------|
| `*mut OpusEncoder cannot be shared between threads safely` | `AudioCodec: Send + Sync` を要求しているが、libopus の状態は単一スレッド前提 |
| `decode_float` の引数型不一致 | `Option<&[u8]>` を渡しているが opus 0.3 は `&[u8]` を取る |

## Decision

### 1. 全プリセットは非圧縮 PCM を使用する

`audio-quality.feature` の「balanced = Opus 128kbps」「high-quality = Opus 256kbps」は実現できない。上記「3」のとおり、どのプリセットのフレームサイズも Opus の有効値ではないためである。

```rust
impl AudioPreset {
    /// このプリセットが使用するコーデック（現時点では全プリセットが PCM）
    pub fn codec_type(&self) -> CodecType;
    /// FEC グループサイズ。None は FEC 無効
    pub fn fec_group_size(&self) -> Option<usize>;
    /// FEC の冗長度（0.25 = 25%）
    pub fn fec_redundancy(&self) -> f32;
}
```

| プリセット | コーデック | FEC グループ | 冗長度 |
|-----------|-----------|-------------|--------|
| zero-latency | PCM | なし | 0% |
| ultra-low-latency | PCM | なし | 0% |
| balanced | PCM | 4 | 25% |
| high-quality | PCM | 8 | 12.5% |

Opus を採用するには次のいずれかが必要であり、どちらも遅延最優先の要件（AGENTS.md）に反する。

| 案 | 影響 |
|----|------|
| 全プリセットのフレームサイズを 120 / 240 に変更 | オーディオデバイスのバッファ長は 2 の冪が前提（`AppConfig::validate` が 32/64/128/256 に制限）。ADR-019 の全バジェットが変わる |
| デバイスフレームを Opus フレームに再パケット化 | 最小 2.5ms の枠組み遅延に加え Opus のルックアヘッドが乗る |

したがって `CodecType::Opus` は「利用可能なコーデック種別」として残すが、どのプリセットも選択しない。ユーザーが明示的に選択する経路の設計は本 ADR の範囲外とする。

### 1-b. Opus 実装のコンパイルエラーは修正する

未使用でも壊れたままにはしない。`AudioCodec` の境界を `Send + Sync` から `Send` に緩め、`decode_float` の引数を修正する。

`Sync` は不要である。共有が必要な箇所は `Arc<Mutex<Box<dyn AudioCodec>>>` を使っており、これは内側が `Send` であれば `Sync` になる。`opus-codec` feature を有効にした CI ジョブを追加し、再び腐らないようにする。

### 2. FEC グループサイズはジッタバッファ段数と一致させる

上記「2」の条件を満たす最大値、すなわち `fec_group_size = jitter_buffer_frames` とする。結果として冗長度は段数から導かれる。

`audio-quality.feature` の 10% / 20% は段数を考慮せずに定めた値であり、balanced では復元が間に合わない。段数側を正とし、feature の記述を冗長度 25% / 12.5% に更新する。

段数 0（zero-latency）および 1（ultra-low-latency）では FEC を無効にする。段数 1 のグループは FEC パケットがデータの複製になり、冗長度 100% で帯域を倍にするだけだからである。これは feature の「FEC OFF」と一致する。

`FEC_GROUP_SIZE`（既定値 4）は `FecEncoder::new()` / `FecDecoder::new()` の既定として残す。プリセット経由の経路は常に `with_group_size` を使う。

### 4. コーデックと FEC は `Connection` が持つ

送信シーケンス番号とトランスポートを所有しているのは `Connection` であり、FEC のグループ番号は送信シーケンスから導出される（`group_sequence = sequence / group_size`、`packet_index = sequence % group_size`）。送信側と受信側でこの対応が一致するため、グループ番号を別途プロトコルに載せる必要がない。

```rust
impl Connection {
    /// 音声の符号化と FEC を設定する。未呼び出し時は PCM・FEC 無効
    pub fn set_audio_encoding(&mut self, config: AudioEncodingConfig) -> Result<(), NetworkError>;
}
```

`send_audio` はコーデックで符号化し、音声パケット送信後に FEC エンコーダへ渡す。グループが揃ったら `PacketType::Fec` パケットを送る。受信ループは `PacketType::Fec` を復号器へ渡し、復元できたパケットを通常の音声コールバックに流す。

### 5. 復元パケットのタイムスタンプは再構成しない

FEC が復元するのはペイロードのみで、失われたパケットのタイムスタンプは失われたままである。復元パケットはタイムスタンプ 0 でコールバックへ渡す。受信経路（ジッタバッファ、デコード、再生）はシーケンス番号で動作しタイムスタンプを参照しないため、現時点で影響はない。タイムスタンプに依存する処理を追加する場合は、`FecPacket` にグループ先頭タイムスタンプを載せる拡張が必要になる。

## Consequences

### メリット

- REQ-AUD-114 / 115 / 116 が `@must` に昇格し、プリセット定義全体が検証対象になる
- FEC が実経路に配線され、復元が再生に間に合う構成になる
- `OpusCodec` がコンパイル可能になり、CI で継続的に検証される

### デメリット・トレードオフ

- balanced / high-quality は帯域を圧縮しない。PCM で 1.54 Mbps/ch を要するため、狭帯域回線では使用できない。これは「帯域効率より遅延削減を優先する」（AGENTS.md）に沿った帰結である
- balanced の冗長度が feature 記載の 10% から 25% に増える。帯域は増えるが、10% では復元が間に合わないため実質的な劣化ではない
- high-quality の冗長度が 20% から 12.5% に下がる。段数 8 に対して 1 パケットの XOR しか送らないため、グループ内 2 個以上のロスは復元できない。Reed-Solomon 化は ADR-006 が将来拡張として挙げている
- 復元パケットのタイムスタンプが 0 になる（上記 5）
- `AudioCodec` から `Sync` を外したため、コーデックを複数スレッドで直接共有する実装はできない（`Mutex` で包む必要がある）

### 未解決の課題

REQ-AUD-102（Opus コーデックを使用する）と REQ-AUD-103（帯域不足で自動変更）は、フレームサイズの非互換が解決するまで実現できない。狭帯域回線への対応を製品要件とするかは別途判断し、必要なら新規 ADR でフレームサイズ戦略を定める。

### 関連ADR

- ADR-003: 音声コーデック選択
- ADR-006: FEC 戦略（グループサイズの矛盾を本 ADR で解消）
- ADR-019: プリセット遅延バジェット
- ADR-020: ジッタバッファの受信経路への配線（FEC の間に合う条件が段数に依存する）
