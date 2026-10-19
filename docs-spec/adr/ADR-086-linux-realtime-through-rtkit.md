---
sidebar_label: "ADR-086: Linux Realtime Through rtkit"
sidebar_position: 86
---

# ADR-086: Linux では音声スレッドの実時間優先度を、取れなければ rtkit に頼んで取る

## Status

Accepted

## Context

Linux の音声スレッドは、`pthread_setschedparam(SCHED_FIFO, 90)` を直接呼ぶだけで実時間の優先度を取っていた。これが通るのは `RLIMIT_RTPRIO` が 0 でない環境
（audio グループと limits.d を手で設定した人、上限を上げて起動した入れ物）だけである。ふつうのデスクトップの上限は 0 なので失敗し、
`nice -20` に落ちたと記録していたが、`RLIMIT_NICE` も 0 なので `setpriority` も失敗しており、戻り値を見ていなかった。実態は通常の優先度のままで、ログは嘘をついていた。

2 コアの Linux 機で PipeWire だけが実時間（rtkit 経由。デスクトップの既定）、アプリの音声スレッドが通常の優先度という条件を作って 2 分ずつ測ると、
送る音の無音区間は直る（全部通常 106〜170 か所 → 9 か所）が、アプリの再生側の xrun が 2 分で 12〜27 回、出力の開き直しが 1〜4 回残った。

ふつうのデスクトップで実時間を取る標準の方法は rtkit（`org.freedesktop.RealtimeKit1`）で、PipeWire・PulseAudio・Firefox が使っている。

## Decision

音声スレッド（セッションのスレッド・送信スレッド・取り込みと再生のコールバックのスレッド）は、自分の優先度を次の順で頼む。上から順に、通ったところで止める。

1. `SCHED_FIFO` 90 を直接。通れば、そのスレッドから起こしたスレッドも同じ優先度を引き継ぐ（従来どおり）。
2. rtkit に `MakeThreadRealtime`。優先度は rtkit の `MaxRealtimePriority` を上限にする（標準は 20）。rtkit の条件である `RLIMIT_RTTIME`
   （実時間のまま止まらずに使える CPU 時間）を、rtkit の `RTTimeUSecMax`（標準は 200 ms）以下の soft・hard にそろえてから呼ぶ。
3. 実時間が取れなければ nice を下げる。直接 `-20`、だめなら rtkit の `MakeThreadHighPriority`（`MinNiceLevel`）。

どれも取れなければ、通常の優先度で動くと警告に出す（取れたか取れなかったかを実際の戻り値で書く）。

- rtkit は `SCHED_RESET_ON_FORK` 付きで与えるので、そのスレッドから起こしたスレッドは優先度を引き継がない。cpal が起こすコールバックのスレッドは、
  自分で作る前に頼めないので、最初のコールバックで自分の番号を取り、別のスレッドから rtkit に頼む（コールバックは D-Bus の返事を待たない）。
  セッションのスレッドは `audio::bounded` で 3 秒だけ待つ（システムバスが応えなくても入室を止めない）。
- rtkit への D-Bus は `zbus`（Rust のみ。libdbus を入れさせない）。アプリはすでにシングルインスタンスのために Linux で zbus を作っている（ADR-085）。
  cpal の `realtime-dbus` は使わない: libdbus に動的に繋がり、`ioplug`（PipeWire の alsa プラグイン）の PCM を対象から外すため、PipeWire の機器では何もしない。
- `RLIMIT_RTTIME` はプロセス全体に掛かる。音声スレッドは周期ごとにデバイスかソケットで止まるので超えない。超えると `SIGXCPU` で止められる。
  rtkit を使う経路でだけ掛ける（直接 `SCHED_FIFO` が通る環境では掛けない）。

## Consequences

- audio グループなどの設定をしていない Linux のデスクトップでも、rtkit があれば音声スレッドが実時間で動く。rtkit が無い環境（入れ物・最小構成）は従来どおり通常の優先度で、警告にそう出る。
- 遅延に足す処理は無い（頼むのはスレッドの始めの 1 回で、コールバックは待たない）。
- 依存が増える（Linux では zbus。アプリはすでに作っている）。第三者ライセンスの表示を作り直す。
- Flatpak などでプロセス名前空間が分かれた環境では、スレッド番号が rtkit の側と食い違う。その場合 rtkit は断り、警告に出るだけで動く。

## Verification

- `audio::priority::rtkit` の単体テストで、`RLIMIT_RTTIME` を rtkit の上限にそろえること・すでに低ければ変えないこと・システムバスが無いときに理由つきで失敗することを確かめる。
- 2 コアの Linux 機（rtkit あり・上限 0 の入れ物）で、直す前後の無音区間・xrun・開き直しを 2 分ずつ測って並べる（`jamjam-soak.py`）。
