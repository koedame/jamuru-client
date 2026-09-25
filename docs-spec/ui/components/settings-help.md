# SettingsHelp（設定の手伝い）

同じルームの参加者の音声の設定を、遠隔でしてあげる機能の画面（[ADR-043](../../adr/ADR-043-remote-operation-rpc.md)、REQ-RMT-001〜008）。
手伝う人が申請し、手伝われる人が許可して始まる。変更は 1 件ずつ申請して、手伝われる人が許可したものだけが適用される。
どちらからでもいつでも止められ、適用された変更はルームのチャットに記録される。

---

## 情報設計

| 情報 | 所属 | 置き場所 |
|------|------|---------|
| 手伝いの入口（「設定を手伝う」） | 参加者ごと | ルームのサイドバーの参加者の行。手伝いを受け取れる（`peer_message` を知らせた）アプリの参加者にだけ出す。自分が誰かを手伝っている（申請中を含む）間は出さない |
| 申請への答え（開始・変更ごと） | 手伝われる人 | 画面の中央の問いかけ（モーダル）。答えるまで何も変わらない。変更の申請は 1 件ずつしか来ない（答える前の次の申請はアプリが捨てる）。変更の問いかけには［手伝いを止める］もあり、モーダルの上からでも止められる |
| 手伝いが続いていること・停止 | ルーム | ミキサーの上の帯。手伝われる側は「◯◯さんが音声の設定を手伝っています」[停止]、手伝う側は申請中なら「◯◯さんの返事を待っています」[取り消す]、手伝い中なら「◯◯さんの設定を手伝っています」[設定を開く][手伝いをやめる] |
| 相手の音声の設定 | 手伝う人 | 右から出る側面パネル（タイトル「◯◯さんの音声の設定」）。中身は設定ウィンドウと同じ Devices タブ。選ぶと申請になり、許可を待つあいだは「◯◯さんの許可を待っています」と出して次の操作を止める。断られた・適用できなかったときは、その理由をパネルに出す |
| 一度きりの知らせ（断られた・相手が止めた・相手が退出した） | ルーム | 帯の位置にトーストで数秒 |
| 変更の記録 | ルーム | チャットの system 行（下記） |

---

## 部品

| 部品 | 種類 | 役割 |
|------|------|------|
| `SettingsHelpQuestion` | Pure | 問いかけ。`message` と「許可」「断る」（と、渡されれば「手伝いを止める」）のボタン。最初のフォーカスは「断る」、Escape は断る。外側のクリックでは閉じない（うっかり答えさせない）。問いかけが出てから `ALLOW_DELAY_MS`（0.5 秒）は「許可」を受け付けない（`aria-disabled`。連打やひとつ前へのクリックで答えさせない）。閉じるとフォーカスを元の場所に戻す。別の問いかけは別のマウントにする: `useSettingsHelp` は問いかけごとに `key`（申請ごとの通し番号と申請の番号）を付けて出ている間だけ描くので、最初の描画から「許可」を受け付けない |
| `SettingsHelpBar` | Pure | 帯。`message`・`status`・操作ボタンの並び |
| `SettingsHelpPanel` | Pure | 相手の設定。`useAudioSettingsTab` が作った Devices タブの props をそのまま描く。`waiting` の間は選べない |
| `useSettingsHelp` | Adapter（hook） | メイン画面のポーリングが届けるイベントから状態を作り、上の部品と入口の判定（`canOffer`）を返す |
| `useAudioSettingsTab` | 共通（hook） | 音声の設定から Devices タブの props を作る。設定ウィンドウと手伝う人のパネルが同じ選択肢を出すための 1 か所 |

`data-testid`: `settings-help-offer`（入口）・`settings-help-question` / `settings-help-allow` / `settings-help-decline` / `settings-help-question-stop`（問いかけ）・
`settings-help-bar` / `settings-help-stop-helped` / `settings-help-stop-helper` / `settings-help-cancel` / `settings-help-open`（帯）・
`settings-help-panel` / `settings-help-panel-status`（パネル）。

---

## 文言

| キー | 日本語 | English |
|------|--------|---------|
| `settingsHelp.offer` | 設定を手伝う | Help with settings |
| `settingsHelp.request.message` | {{name}}さんが音声の設定を手伝いたいそうです | {{name}} wants to help with your audio settings |
| `settingsHelp.proposal.message` | {{name}}さんが{{setting}}を「{{value}}」に変えようとしています | {{name}} wants to change your {{setting}} to {{value}} |
| `settingsHelp.helped.bar` / `.stop` | {{name}}さんが音声の設定を手伝っています / 停止 | {{name}} is helping with your audio settings / Stop |
| `chat.system.settingsHelpChanged` | {{helper}}さんが{{helped}}さんの{{setting}}設定を変更しました | {{helper}} changed {{helped}}'s {{setting}} setting |
| `settingsHelp.proposal.stop` | 手伝いを止める | Stop help |
| `settingsHelp.helper.refused.device_gone` | その機器は{{name}}さんのところで接続されていないため、変更できませんでした | {{name}}'s app could not switch: that device is no longer connected |
| `settingsHelp.helper.refused.invalid_value` | {{name}}さんのところではその値を使えないため、変更できませんでした | {{name}}'s app could not use that value |
| `settingsHelp.helper.refused.unavailable` | {{name}}さんのところで変更を保存できませんでした | {{name}}'s app could not save the change |

変更の申請で見せる値（`{{value}}`）は、手伝われる人自身のアプリが組み立てる。デバイスは手伝う人に見せた一覧での名前
（アプリが申請と一緒に渡す。名前の無い機器は「接続されていない機器」。ID は出さない）、チャンネルは設定ウィンドウと同じ表記（「チャンネル 3」「なし」）。
手伝う人のアプリにはデバイス ID を渡さない（REQ-RMT-006）。

---

## チャットの記録

`ChatMessage` の system 行に 3 種類を足した。本文は持たず、受け取ったアプリが自分の言語で組み立てる。

| `system_kind` | 文 | 使う値 |
|---------------|----|--------|
| `settings_help_started` | {{helper}}さんが{{helped}}さんの音声の設定の手伝いを始めました | `helper_name`・`sender_name`（手伝われる人） |
| `settings_help_changed` | {{helper}}さんが{{helped}}さんの{{setting}}設定を変更しました | 上に加えて `setting`（設定の名前。`input_device` など） |
| `settings_help_ended` | {{helper}}さんによる{{helped}}さんの設定の手伝いが終わりました | `helper_name`・`sender_name` |

記録はルームの全員（`peer_message` を知らせた参加者）に届く。送り主（手伝われる人）はサーバーが付け、手伝う人の名前
（`helper_name`）は受け取ったアプリがルームの参加者から引く。ルームにいたことの無い人を名指しした記録は出さない。
終了の記録は、どちらかが止めたときに出る。退出・切断で終わったときは、退出の記録がそれに代わる。

---

## 関連

- [settings-panel.md](./settings-panel.md) — 同じ Devices タブ・同じ `settings_change`
- [chat-panel.md](./chat-panel.md) — system 行
- [signaling.md](../../api/signaling.md) — `PeerMessage` と `features`
