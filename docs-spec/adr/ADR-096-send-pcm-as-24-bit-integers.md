# ADR-096: PCM は 24-bit 符号付き整数で送る

## Status

Accepted。[ADR-003](./ADR-003-audio-codec.md)・[ADR-008](./ADR-008-zero-latency-mode.md) が「PCM 32-bit float」としていたワイヤ上の形式を置き換える。

[ADR-111](./ADR-111-pack-audio-packets-without-loss.md) が、この 24-bit の値をパケットごとに損失なく詰めて送る形に替えた（値は変わらない。プロトコル版は 3）。

## Context

jamuru は PCM を 32-bit float のまま送っていた。ステレオで 1 本あたり 3.5〜4.1 Mbps（[ADR-077](./ADR-077-every-participant-is-linked.md)）で、16 人のルームでは 1 人の上りが 52〜61 Mbps になる。

競合の最高音質は 48 kHz / 16-bit の非圧縮で、1 本あたり約 1.5 Mbps に収まる。一方 jamuru は音質を犠牲にしない（AGENTS.md の最優先要件）ので、16-bit まで落とす理由は無い。

オーディオインターフェースの AD/DA は最大で 24-bit で、実際の S/N は 24-bit の理論値（約 146 dB）よりずっと低い（良い機材でフルスケールの約 120 dB 下）。32-bit float の仮数部も 24 bit なので、24-bit 整数で送っても、フルスケール近くでは float の分解能とほぼ同じ。

## Decision

1. **PCM は 1 サンプルを 24-bit 符号付き整数（リトルエンディアン 3 バイト）で送る。** 値は `round(x × 2^23)`。受け取る側は `v / 2^23` で float に戻す。32-bit float の形式は残さない（1.0.0 までは破壊的変更をしてよい）。
2. **±1.0 を超えるサンプルは、24-bit で表せる最も近い値に切り詰めて送り、`ConnectionStats::samples_clipped` に数える。** +1.0 ちょうどは 8388607（1 歩手前）になるが、範囲内なので数えない。NaN は無音で送り、数える。入力のリミッター（[ADR-094](./ADR-094-limiters-on-what-is-sent-and-what-is-played.md)）が入っていれば、送る音は 0 dBFS を超えず、ここで数えるのはリミッターを切ったときと、リミッターの手前を通らない音だけになる。アプリは 1 つのリンクで初めて切り詰めたとき、ログに 1 回だけ警告する。
3. **プロトコル版を 1 から 2 に上げる。** 32-bit float と 24-bit 整数にはワイヤ上の目印が無く、取り違えると大きな雑音になる。古い版の相手のパケットは受け付けず、鍵の交換も通らないので、繋がらない。
4. **1 パケットに載るサンプル数と必要帯域は、3 バイトで数える**（`PCM_BYTES_PER_SAMPLE`）。256 フレームのステレオは、1536 バイトになるのでこれまでどおり 2 パケットに分ける（[ADR-070](./ADR-070-split-audio-frames-that-do-not-fit-a-packet.md)）。

却下した案:

- 16-bit: 帯域はさらに 3 分の 1 減るが、ダイナミックレンジが約 96 dB に落ち、音質を犠牲にしない方針に反する。
- 32-bit float と 24-bit の両方を残して選ばせる: 古い形式の読み替えを残すことになり、取り違えの危険も残る。
- 切り詰めずに折り返す: 0 dBFS を少し超えた音が、反対の符号の大きな音になる。

## Consequences

- 1 本あたりの必要帯域が約 22 % 減る。ステレオ・48 kHz で zero-latency 3.07、ultra-low-latency 2.69、balanced 3.12、high-quality 2.81 Mbps。16 人のルームの上り・下りは 1 人あたり 40〜47 Mbps（ADR-077 の表）。
- 遅延は変わらない。遅延はフレームの長さとジッタバッファの段数で決まり、ペイロードの大きさには依存しない。符号化は 1 サンプルあたり乗算・丸め・3 バイト書き込みだけで、受信も整数 3 バイトの読み込みだけ。
- 音質の変化は、24-bit の丸めだけ。0.99 フルスケールの正弦波で、丸め雑音は信号の約 145 dB 下（測定: `tests/audio_quality_test.rs`）。ASIO の 32-bit 整数や float の入力が持つ 24-bit より下の桁は送られない。
- 古い版のアプリとは繋がらない。ステージングのサーバー側の音質チェックのボットとエコーは、同じタグで出し直す。
- ワイヤ上に目印が無いので、将来さらに形式を変えるときもプロトコル版を上げる。

## References

- [ADR-003](./ADR-003-audio-codec.md)・[ADR-008](./ADR-008-zero-latency-mode.md)・[ADR-021](./ADR-021-preset-codec-and-fec.md): 非圧縮 PCM
- [ADR-070](./ADR-070-split-audio-frames-that-do-not-fit-a-packet.md): 1 パケットに収まらないフレームの分割
- REQ-AUD-044・REQ-AUD-045（[requirements.md](../requirements.md)）
