---
sidebar_label: "ADR-087: Windows Audio Threads Join MMCSS"
sidebar_position: 87
---

# ADR-087: Windows では音声スレッドを MMCSS に登録して実時間の範囲を取る

## Status

Accepted

## Context

Windows の音声スレッドは、セッションのスレッドと送信スレッドが `SetPriorityClass(HIGH)` と `SetThreadPriority(TIME_CRITICAL)` を呼ぶだけだった。
これはふつうの範囲（1〜15）の最上位（15）で、実時間の範囲（16〜31）ではない。それでもログは `real-time priority enabled (HIGH + TIME_CRITICAL)` と書いていた。
さらに、デバイスと音をやり取りするスレッド（cpal が WASAPI の取り込みと再生のために起こすスレッド）は何も頼んでおらず、プロセスのクラスから決まる 13 前後で動いていた
（cpal 0.18 はこのスレッドを `realtime` 機能のときだけ上げるが、アプリは有効にしていない）。

macOS（Mach の `TIME_CONSTRAINT`）と Linux（ADR-086 の rtkit）は、ふつうのアプリが権限なしで頼める OS の窓口を使っている。Windows にはその窓口がある。MMCSS（Multimedia Class Scheduler Service）で、
`AvSetMmThreadCharacteristicsW` で呼ぶスレッドをタスクに登録すると、サービスがそのスレッドの優先度を実時間の範囲に上げる。

## Decision

音声スレッド（セッションのスレッド・送信スレッド・取り込みと再生のコールバックのスレッド）は、自分で MMCSS のタスク "Pro Audio" に登録し、タスクの中の優先度も `AVRT_PRIORITY_CRITICAL` に上げる。

- **タスクは "Pro Audio"**。Windows が標準で持つ 2 つのタスクは、"Audio" が Scheduling Category = Medium・Background Only = True（背景で鳴らす音向け）、
  "Pro Audio" が Scheduling Category = High・Background Only = False（前面で動く、遅延に厳しい音向け）。リアルタイムの合奏は後者に当たる
  （Windows 11 の既定のレジストリ `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile\Tasks` で確かめた）。
- **cpal の `realtime` 機能は使わない。** Windows では `audio_thread_priority` を足し（第三者ライセンスの表示を作り直す）、cpal が起こす 2 つのスレッドしか覆わない。
  セッションと送信のスレッドは別に頼む必要があり、取れたか取れなかったかをアプリのログに出すのも別にいる。コールバックのスレッドは、cpal がループに入る前に上げるのと同じスレッドで
  最初のコールバックから 1 回頼む（Linux と同じ作り。`promote_callback_thread`）。MMCSS への登録は呼んだスレッドだけが対象なので、別のスレッドに任せることはできない。
- **断られたとき**（サービスが無効にされている等。止まっているだけなら、呼び出しがサービスを起こす）は、`THREAD_PRIORITY_TIME_CRITICAL`（ふつうの範囲の最上位）に落とし、**実時間ではないとログに出す**。それも失敗したら通常の優先度と出す。
  取れたもの以外を「real-time」と書かない。MMCSS に登録したスレッドには `SetThreadPriority` を呼ばない（サービスが管理する）。
- **ASIO は対象にしない。** ASIO のコールバックはドライバが持つスレッドで動き、ドライバが自分で優先度を決める。アプリが呼ばないので登録しない（セッションと送信のスレッドは自分のものなので登録する）。
- プロセスのクラス `HIGH_PRIORITY_CLASS` は残す。MMCSS に登録していないスレッド（ネットワーク・タイマー）が、登録したスレッドの下で 8 ではなく 13 で動くため。
  登録したスレッドの優先度はクラスに依らず 26 のままで、クラスを Normal に戻した測定（有線の相手役・ほかに何も動かしていない端末）では無音区間・受信の詰まりに差が出なかった。
  差が無いので、従来の挙動（負荷のあるマシンで登録していないスレッドが遅れない側）を残した。

## Consequences

- Windows のコールバックのスレッドとセッション・送信のスレッドが、MMCSS を通して実時間の範囲で動く。MMCSS が無い環境（サービスを止めた構成）では従来と同じ最上位 15 で、ログにそう出る。
- 遅延に足す処理は無い（頼むのはスレッドの始めの 1 回）。MMCSS の呼び出しはローカルのサービスへの RPC で、サービスが止まっていれば即座に失敗する。
- 依存は増えない（`windows` クレートの `Win32_System_Threading` に `Av*` が入っている）。

## Verification

- Windows の端末（u-dev-win）で、直す前後の同じ測定（2 分 × 3 回以上、有線の相手役）を並べる。取れた優先度は、動いているアプリのスレッドの `CurrentPriority` で確かめる。
- MMCSS のサービスを無効にした状態でアプリを起動し、ログが「実時間ではない」と出して音が流れ続けることを確かめる（止めただけでは、呼び出しがサービスを起こして登録できる）。
