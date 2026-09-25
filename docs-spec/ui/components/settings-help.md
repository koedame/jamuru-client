# SettingsHelp（設定の手伝い）

同じルームの参加者の音声の設定を、遠隔でしてあげる機能の画面（[ADR-044](../../adr/ADR-044-portals-and-permissions.md) §5、REQ-RMT-001〜008・030）。
手伝う人が申し出て、手伝われる人が**初めに 1 回**許可すると始まる。許可のあとは、手伝う人の手元に手伝われる人のアプリと同じ画面が
別のウィンドウで開き、そこでの操作が相手のアプリに届く。1 つ 1 つの操作は聞かれない。どちらからでもいつでも止められ、
音声の設定の変更はルームのチャットに記録される。

---

## 情報設計

| 情報 | 所属 | 置き場所 |
|------|------|---------|
| 手伝いの入口（「設定を手伝う」） | 参加者ごと | ルームのサイドバーの参加者の行。手伝いを受け取れる（`peer_message` を知らせた）アプリの参加者にだけ出す。自分が誰かを手伝っている（申し出中を含む）間は出さない |
| 申し出への答え（初めの 1 回だけ） | 手伝われる人 | 画面の中央の問いかけ（モーダル）。答えるまで何も変わらない。許可を押すとすぐ閉じ、手伝いが始まると帯が出る |
| 手伝いが続いていること・停止 | ルーム | ミキサーの上の帯。手伝われる側は「◯◯さんが音声の設定を手伝っています」[停止]、手伝う側は申し出中なら「◯◯さんの返事を待っています」[取り消す]、手伝い中なら「◯◯さんの設定を手伝っています」[手伝いをやめる]。手伝いのあいだ出続ける |
| 手伝う人の窓 | 手伝う人 | 別のウィンドウ（`HelperScreen`）。上に「◯◯さんのアプリを操作しています。ここで変えたことは、◯◯さんのアプリに反映されます。」の帯、その下に手伝われる人のアプリの画面（`MainScreen`）。窓を閉じると手伝いが止まる |
| 相手の音声の設定 | 手伝う人 | 窓のヘッダーの設定ボタンで右から出る側面パネル（タイトル「◯◯さんの音声の設定」）。中身は設定ウィンドウと同じ Devices タブ。選ぶとその 1 件が相手のアプリに適用され、返った設定が出る。適用できなかったときは、その理由をパネルに出す |
| 一度きりの知らせ（断られた・相手が止めた・相手が退出した） | ルーム | 帯の位置にトーストで数秒 |
| 変更の記録 | ルーム | チャットの system 行（下記） |

手伝う人の窓に**出さない**もの: 退室・チャットの送信欄・参加者への手伝いの申し出。相手のアプリはこれらを断るが、押して断られる操作は見せない。
相手の入ったことのあるルームの履歴と接続先は、窓は読みに行かない。

---

## 部品

| 部品 | 種類 | 役割 |
|------|------|------|
| `SettingsHelpQuestion` | Pure | 問いかけ。`message` と「許可」「断る」のボタン。最初のフォーカスは「断る」、Escape は断る。外側のクリックでは閉じない（うっかり答えさせない）。問いかけが出てから `ALLOW_DELAY_MS`（0.5 秒）は「許可」を受け付けない（`aria-disabled`。連打やひとつ前へのクリックで答えさせない）。閉じるとフォーカスを元の場所に戻す。別の問いかけは別のマウントにする: `useSettingsHelp` は申し出ごとに `key`（申し出ごとの通し番号）を付けて出ている間だけ描くので、最初の描画から「許可」を受け付けない |
| `SettingsHelpBar` | Pure | 帯。`message`・操作ボタンの並び |
| `SettingsHelpPanel` | Pure | 相手の設定。`useAudioSettingsTab` が作った Devices タブの props をそのまま描く。`waiting`（変更が相手のアプリへ向かっている間）は選べない |
| `useSettingsHelp` | Adapter（hook） | バックエンドが送るイベント（`session:settings-help`）から状態を作り、上の部品と入口の判定（`canOffer`）を返す |
| `HelperScreen` | Adapter（画面） | 手伝う人の窓。`MainScreen`（`helper` を渡す）と設定パネルを、手伝われる人のアプリに繋いで描く。相手の設定は `settings_get`・`settings_change` とイベント `audio:config-changed`（番号の新しいほうを採る）で追う |
| `useAudioSettingsTab` | 共通（hook） | 音声の設定から Devices タブの props を作る。設定ウィンドウと手伝う人のパネルが同じ選択肢を出すための 1 か所 |

窓は、画面から見たバックエンドの入口（`lib/backend.ts`）を `lib/helperBackend.ts` に差し替えて動く。呼びは相手のアプリへ（`help_call`）、
イベントは相手のアプリのものが届く。窓自身のものは、言語・ログ・大きさ（`config_get_language` など）だけ。相手のアプリが呼びを断ると
（`denied`）、呼びの失敗として届く。

`data-testid`: `settings-help-offer`（入口）・`settings-help-question` / `settings-help-allow` / `settings-help-decline`（問いかけ）・
`settings-help-bar` / `settings-help-stop-helped` / `settings-help-stop-helper` / `settings-help-cancel`（帯）・
`settings-help-window` / `settings-help-window-banner` / `settings-help-window-waiting`（手伝う人の窓）・
`settings-help-panel` / `settings-help-panel-status`（パネル）。

---

## 文言

| キー | 日本語 | English |
|------|--------|---------|
| `settingsHelp.offer` | 設定を手伝う | Help with settings |
| `settingsHelp.request.message` | {{name}}さんが音声の設定を手伝いたいそうです | {{name}} wants to help with your audio settings |
| `settingsHelp.helped.bar` / `.stop` | {{name}}さんが音声の設定を手伝っています / 停止 | {{name}} is helping with your audio settings / Stop |
| `settingsHelp.window.banner` | {{name}}さんのアプリを操作しています。ここで変えたことは、{{name}}さんのアプリに反映されます。 | You are working in {{name}}'s app. What you change here changes theirs. |
| `chat.system.settingsHelpChanged` | {{helper}}さんが{{helped}}さんの{{setting}}設定を変更しました | {{helper}} changed {{helped}}'s {{setting}} setting |
| `settingsHelp.helper.refused.device_gone` | その機器は{{name}}さんのところで接続されていないため、変更できませんでした | {{name}}'s app could not switch: that device is no longer connected |
| `settingsHelp.helper.refused.invalid_value` | {{name}}さんのところではその値を使えないため、変更できませんでした | {{name}}'s app could not use that value |
| `settingsHelp.helper.refused.unavailable` | {{name}}さんのところで変更を保存できませんでした | {{name}}'s app could not save the change |

手伝う人のアプリにはデバイス ID を渡さない（REQ-RMT-006）。機器は名前で見え、選ぶときは手伝いのあいだだけの仮の名前（`input-N` / `output-N`）を使う。

---

## チャットの記録

`ChatMessage` の system 行に 3 種類を足した。本文は持たず、受け取ったアプリが自分の言語で組み立てる。

| `system_kind` | 文 | 使う値 |
|---------------|----|--------|
| `settings_help_started` | {{helper}}さんが{{helped}}さんの音声の設定の手伝いを始めました | `helper_name`・`sender_name`（手伝われる人） |
| `settings_help_changed` | {{helper}}さんが{{helped}}さんの{{setting}}設定を変更しました | 上に加えて `setting`（設定の名前。`input_device` など） |
| `settings_help_ended` | {{helper}}さんによる{{helped}}さんの設定の手伝いが終わりました | `helper_name`・`sender_name` |

記録はルームの全員（`peer_message` を知らせた参加者）に届く。ミュート・音量・パンの変更は記録しない。送り主（手伝われる人）はサーバーが付け、
手伝う人の名前（`helper_name`）は受け取ったアプリがルームの参加者から引く。ルームにいたことの無い人を名指しした記録は出さない。
終了の記録は、どちらかが止めたときに出る。退出・切断で終わったときは、退出の記録がそれに代わる。

---

## 関連

- [settings-panel.md](./settings-panel.md) — 同じ Devices タブ・同じ `settings_change`
- [chat-panel.md](./chat-panel.md) — system 行
- [signaling.md](../../api/signaling.md) — `PeerMessage` と `features`
- [remote-permissions.md](../../api/remote-permissions.md) — 手伝う人が相手のアプリにできる操作の一覧
