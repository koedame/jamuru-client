---
sidebar_label: "ADR-073: Playback Opens at the Device Rate"
sidebar_position: 73
---

# ADR-073: macOS では、出力デバイスが動いているレートで開き、セッションのレートから変換する

## Status

Accepted

## Context

macOS（Apple 製チップ、0.1.0-89 beta）で、システムの既定の入出力が AirPods Pro のとき、入室のたびに約 3 秒後に音声が止まった（3 回中 3 回）。ログは次のとおり。

```
Capture opens at 24000 Hz and is converted to the session's 48000 Hz
Starting playback on device: system default
ERROR Audio streaming failed: Failed to start playback: Stream error: Sample rate update timed out
```

1. 入力は、デバイスが申告するレートで開いて変換する（[ADR-052](./ADR-052-capture-is-converted-when-the-device-cannot-open-at-the-session-rate.md)）ので開けた。出力は、セッションの 48000 Hz のまま開いていた。
2. cpal 0.18 の CoreAudio は、ストリームを開くとき、デバイスの公称サンプルレートをストリームのレートに書き換え、変化の通知を待つ（待ちの上限は呼び出し側の指定か 1 秒）。
3. AirPods はマイクが使われている間は通話用（HFP）の接続になり、公称レートが 24000 Hz などに固定される。48000 Hz への書き換えは通知されず、待ちが切れて開けない。申告するレートの一覧には 48000 Hz が残るので、一覧から選ぶ方法（`input_open_rate`）では避けられない。

## Decision

1. **macOS では、出力デバイスが今動いているレートで開く**（`output_open_rate`。cpal の既定の構成のレート）。他の OS は、入力と同じく、申告するレートからセッションのレートに最も近いものを選ぶ。
2. **セッションのレートと違うときは、再生のコールバックの中で、メモリを確保せずに、セッションのレートから変換する。** 入力と同じ変換（`CaptureResampler`。ADR-052）を、音源が 1 フレームを作り終えるたびに通す（`FramePuller::converting`）。デバイスがセッションのレートで動いているときは変換しない。
3. **デバイスのサンプルレートは書き換えない。** 他のアプリが同じデバイスを使っている間も、そのアプリの音を変えない。

## Consequences

- REQ-AUD-127 の検証は、レート選びと変換（音程・チャンネル・バッファが増えないこと）の単体テストで行う。AirPods の実機では確かめていない。
- 44100 Hz などで動いているデバイスは、これまで jamuru がデバイスのレートを 48000 Hz に書き換えていたが、書き換えずに変換する。変換の遅延は 0.3 ms 未満（ADR-052）。48000 Hz に揃えたい利用者は、Audio MIDI 設定でデバイスの形式を 48000 Hz にすれば、変換は入らない。
- 再生が開けなかったときは、これまでどおり画面に「出力デバイスを開けませんでした（名前）」が出る。
