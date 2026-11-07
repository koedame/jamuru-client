---
sidebar_label: "ADR-116: Tell the OS the Session Is Latency-Critical"
sidebar_position: 116
---

# ADR-116: セッションの間、OS に「遅れてはいけない音声のアプリだ」と伝える

## Status

Accepted。

## Context

OS は電力を節約するために、背景の仕事を後回しにする。窓を隠した・最小化した音声のアプリは、OS から見ると背景の仕事に見える。
音声のスレッドの優先度は上げてある（macOS の `TIME_CONSTRAINT`、Windows の MMCSS、Linux の `SCHED_FIFO` と rtkit。ADR-086・ADR-087）が、それとは別に、プロセス全体に効く節約がある。

- **macOS: App Nap。** 窓が隠れたアプリのタイマーをまとめて遅らせ、CPU の割り当てを下げる。
- **Windows: タイマーの粗さ。** システムのタイマーは既定で 15.6 ms の刻みで、`tokio::time::sleep` のような待ちはその刻みに切り上がる。
- **Windows 11: EcoQoS とタイマーの分解能の取り上げ。** 窓が最小化された（隠れた）プロセスは、省電力の指定（EcoQoS）で遅く動かされ、`timeBeginPeriod` で上げたタイマーの分解能も無視される。
- **全 OS: 音声の UDP ソケットのバッファ。** 音声のソケットは全員のリンクが共有する 1 本（ADR-093）。受信するスレッドが数十 ms 離れた間に届いたパケットは、OS の受信バッファに溜まる。バッファが小さいと、あふれた分は黙って捨てられる。

## Decision

セッションが続いている間だけ、次を OS に頼む。セッションが終わる（退室・終了・音声の開始の失敗）と取り下げる。OS が断ったときはログに残し、頼まなかったものとして動き続ける。

| OS | 頼むこと | API |
|----|----------|-----|
| macOS | App Nap を止め、タイマーをまとめさせない | `NSProcessInfo.beginActivityWithOptions:` に `NSActivityUserInitiated \| NSActivityLatencyCritical`。終わりに `endActivity:` |
| Windows | システムのタイマーを 1 ms にする | `timeBeginPeriod(1)` と `timeEndPeriod(1)` |
| Windows | プロセスを EcoQoS から外し、タイマーの分解能を取り上げさせない | `SetProcessInformation(ProcessPowerThrottling)`。`EXECUTION_SPEED` と `IGNORE_TIMER_RESOLUTION` の制御ビットを立て、状態ビットを 0 にする。終わりに制御ビットを 0 にして OS の判断に戻す |
| 全 OS | 音声のソケットの受信・送信バッファを 1 MiB にする | `SO_RCVBUF` / `SO_SNDBUF`（`set_recv_buffer_size` / `set_send_buffer_size`） |

- プロセスの設定なので、音声・ネットワークの全スレッドに効く。スレッドごとの指定（`SetThreadInformation`）は足さない。
- `NSActivityUserInitiated` は、セッション中にシステムがアイドルでスリープするのも止める。音声のセッションはスリープすれば切れるので、これでよい。
- バッファの大きさは、OS が丸めた値になる（Linux は `net.core.rmem_max` で頭打ち、倍に数える）。ログに頼む前と後の値を出す。

### 足さないもの

- **プロセスを REALTIME の優先度クラスに上げること。** 他のアプリと OS を巻き込む。MMCSS "Pro Audio" と HIGH のクラス（ADR-087）のままにする。
- **Windows の Wi-Fi の media streaming mode**（`WlanSetInterface`）。Wi-Fi でだけ裏の電波探索を減らす指定だが、Windows の端末を Wi-Fi につないだ測定ができておらず（有線の端末しか無い）、効くかを測れていない。OS の Wi-Fi の設定を変える操作なので、測って効き目を確かめ、終了・異常終了で戻ることを確かめてから足す（別のチケットで扱う）。

### デバッグ用の切り替え

ベータ版（遠隔デバッグ）に、前後を同じ版で比べるための口を 2 つ足す。リリースのビルドには入らない。

- `debug.os_hints`: 頼むことのうち外すものを選ぶ（次のセッションから効く）。
- `debug.window_state`: メイン画面を最小化・隠す・戻す。
- `debug.perf` に、短い待ちが実際にかかった時間（`receive_loop_wait`、`send_wait`）を足した。

## Consequences

- Windows でタイマーに待ちが丸められる処理（`tokio::time::sleep` を使うストリーミングのループなど）が、窓を隠しても遅れにくくなる。
- macOS で窓を隠しても、タイマーとネットワークがまとめられない。
- 受信するスレッドが離れても、パケットを落としにくくなる。バッファは各ソケットで 1 MiB × 2 を使うだけで、パケットごとの処理は増えない。
- 電力: セッション中は OS の節約が効かない分、電力は増える。音声のセッション中に限る。
- 依存: macOS に `objc2-foundation`（すでに依存の木に入っている）を直接足し、Windows の `windows` crate に `Win32_Media` を足した。第三者ライセンスの表示は作り直した。

## 測定

### 短い待ちの実際（Windows 11 ビルド 26200、有線の端末）

`tests/timer_wait_measure.rs`（4000 回ずつ）。

| 待ち | システムのタイマーが既定 | `timeBeginPeriod(1)` |
|---|---|---|
| `thread::sleep(100 µs)`（送信スレッド） | 平均 558 µs、p99 709 µs | 平均 549 µs、p99 684 µs |
| `tokio::time::sleep(250 µs)`（ストリーミングのループ） | 平均 16052 µs、p99 16398 µs | 平均 1483 µs、p99 2628 µs |

送信スレッドの待ちは標準ライブラリが高分解能のタイマーを使うので、変わらない。ストリーミングのループの待ちは、頼まないと約 16 ms になる。

### 音声のソケットの受信バッファ

`tests/socket_buffer_measure.rs`。16 人分（1 人 750 パケット/秒、420 バイト、計 12000 パケット/秒）を、受信スレッドが 500 ms ごとに一定時間離れる受信側へ流し、3 秒間の落ちた数を数えた。

Windows 11（既定 65536 バイト → 1048576 バイト）、36000 パケット中に落ちた数:

| 離れた時間 | 既定のバッファ | 1 MiB |
|---|---|---|
| 5 ms | 0 | 0 |
| 10 ms | 0 | 0 |
| 20 ms | 433 | 0 |
| 40 ms | 1622 | 0 |
| 80 ms | 4032 | 0 |
| 160 ms | 7074 | 0 |

## 検証

- Windows: `PROCESS_POWER_THROTTLING_STATE` を `GetProcessInformation` で読み戻し、頼んだ間は制御ビットが立って状態ビットが 0、取り下げたあとは両方 0 に戻ることをテストにした（`src/audio_session/os_hints.rs`。実機の Windows 11 で通した）。
- 全 OS: 受信バッファが頼む前より小さくならないこと、頼まないと変わらないことをテストにした。
