---
sidebar_label: "ADR-116: Widen the Audio Socket Buffers"
sidebar_position: 116
---

# ADR-116: 音声のソケットのバッファを広げる。OS への「遅延に厳しいアプリだ」という申告は、測って要らなかった

## Status

Accepted。

## Context

OS は電力を節約するために背景の仕事を後回しにする。窓を隠した・最小化した音声のアプリは、OS から見ると背景の仕事に見える。
音声のスレッドの優先度は上げてある（macOS の `TIME_CONSTRAINT`、Windows の MMCSS、Linux の `SCHED_FIFO` と rtkit。ADR-086・ADR-087）。
それとは別に、プロセス全体に効く次の申告があり、効くかを測った。

- **macOS: App Nap を止める活動。** `NSProcessInfo.beginActivityWithOptions:` に `NSActivityUserInitiated | NSActivityLatencyCritical`。
- **Windows: システムのタイマーを 1 ms にする。** `timeBeginPeriod(1)`。
- **Windows 11: EcoQoS とタイマーの分解能の取り上げを外す。** `SetProcessInformation(ProcessPowerThrottling)`。
- **Windows: Wi-Fi の media streaming mode。** `WlanSetInterface`。
- **全 OS: 音声の UDP ソケットのバッファを広げる。** 音声のソケットは全員のリンクが共有する 1 本（ADR-093）。受信するスレッドが数十 ms 離れた間に届いたパケットは OS の受信バッファに溜まり、あふれた分は黙って捨てられる。

## Decision

- **足す: 音声のソケットの受信・送信バッファを 1 MiB にする**（`SO_RCVBUF` / `SO_SNDBUF`）。バッファの大きさは OS が丸めた値になる（Linux は `net.core.rmem_max` で頭打ち、倍に数える）。頼む前と後の値をログに出し、OS が断ったときは警告を出してそのまま動く。
- **足さない: 上のほかの申告。** 実機で前後を比べて、遅延にも途切れにも差が出なかった（下の測定）。効かないものを足すと、電力と依存（macOS は `objc2-foundation`、Windows は `windows` crate の `Win32_Media`）だけが増える。
- **足さない: プロセスを REALTIME の優先度クラスに上げること。** 他のアプリと OS を巻き込む。MMCSS "Pro Audio" と HIGH のクラス（ADR-087）のままにする。
- **足さない: Wi-Fi の media streaming mode。** Windows の端末を Wi-Fi につないだ測定ができておらず（有線の端末しか無い）、効くかを測れていない。OS の Wi-Fi の設定を変える操作なので、測って効き目を確かめ、終了・異常終了で戻ることを確かめてから足す。

## Consequences

- 受信するスレッドが数十〜百数十 ms 離れても、パケットを落としにくくなる。各ソケットで 1 MiB × 2 を使うだけで、パケットごとの処理は増えない。
- 複数人（16 人）のときに効く。2 人の通常のセッションでは、前後で数字は変わらない。
- 窓を隠したときの App Nap・タイマーの粗さ・EcoQoS には何もしない。下の測定のとおり、音声が流れている間は音の遅延に影響が出なかった。バッテリーで動く Windows のノートでの EcoQoS は測れていない。

## 測定

### 音声のソケットの受信バッファ

`tests/socket_buffer_measure.rs`（手で回す）。16 人分（1 人 750 パケット/秒、420 バイト、計 12000 パケット/秒）を、受信スレッドが 500 ms ごとに一定時間離れる受信側へ流し、3 秒間の落ちた数を数えた。
Windows 11（既定 65536 バイト → 1048576 バイト）、36000 パケット中に落ちた数:

| 離れた時間 | 既定のバッファ | 1 MiB |
|---|---|---|
| 5 ms | 0 | 0 |
| 10 ms | 0 | 0 |
| 20 ms | 433 | 0 |
| 40 ms | 1622 | 0 |
| 80 ms | 4032 | 0 |
| 160 ms | 7074 | 0 |

### 測って要らなかった申告

相手役（Linux、有線、128 フレーム）に端末が 1000 Hz を 30 秒送る回を、申告ありとなしで交互に 5〜6 回ずつ測った。申告は `debug.os_hints` でセッションごとに外した（測定用のベータ版 beta.222 だけにあった口で、取り除いた）。

| 端末と条件 | 申告 | 回数 | 無音のあった回 | 受け側 starved 中央値 | 端末の往復 `roundtrip_total_ms` 中央値 | 端末の Jitter buffer |
|---|---|---|---|---|---|---|
| Windows 11（有線）、窓が見える | 全部 | 6 | 0 | 0 | - | - |
| 同上 | 外した | 6 | 0 | 0 | - | - |
| Windows 11（有線）、窓を最小化 | 全部 | 5 | 1 | 1 | 23.19 ms | 2.67 ms |
| 同上 | 外した | 5 | 2 | 1 | 23.08 ms | 2.67 ms |
| macOS（有線）、窓を隠す | 全部 | 5 | 0 | 2 | 4.26 ms | 2.67 ms |
| 同上 | 外した | 5 | 0 | 3 | 4.24 ms | 2.67 ms |

差は測定のばらつきの内（往復の差は 0.1 ms 以下、無音も starved も同じ）。Windows のシステムのタイマーだけは、端末の受信ループの待ちが 2.47 ms → 1.18 ms と短くなった。ただしこのループは命令と統計だけを扱い、音は端末のコールバックが再生バッファから取るので（ADR-028）、音声の遅延には出なかった。

## 検証

- 受信バッファが頼む前より小さくならないことをテストにした（`src/audio_session/socket_buffers.rs`）。
