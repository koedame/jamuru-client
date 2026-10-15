# SettingsHelp（設定の手伝い）

同じルームの参加者の音声の設定を、手伝ってもらう機能の画面（[ADR-044](../../adr/ADR-044-portals-and-permissions.md) §5、REQ-RMT-001〜008・030・034〜037）。
手伝ってほしい人が「設定依頼をする」から始め、相手を選んで頼む。頼まれた人が「手伝う」と答えると始まる。始まったあとは、手伝う人の手元に手伝われる人のアプリと同じ画面が
別のウィンドウで開き、そこでの操作が相手のアプリに届く。1 つ 1 つの操作は聞かれない。どちらからでもいつでも止められ、
音声の設定の変更はルームのチャットに記録される。手伝う人がマイクのミュートを解除することは止めず、手伝われる人にすぐ知らせて記録に残す。
手伝いが終わると、変わった設定を示して 1 回で手伝いの前に戻せる。何も変えないまま 30 分たつと自動で終わり、残り時間は両方の画面に出る。

---

## 情報設計

| 情報 | 所属 | 置き場所 |
|------|------|---------|
| 手伝いの入口（「設定依頼をする」） | 手伝われたい人 | ルームのサイドバーの参加者の一覧の下。自分が手伝われている（頼んで返事を待っている間を含む）間は出さない。手伝う人の窓には出さない |
| 依頼の中身と相手選び | 手伝われたい人 | 画面の中央のダイアログ（`SettingsHelpAsk`）。1 つ目は、何をするか・見えるのは jamuru の中だけ・いつでも止められる、の 3 行。2 つ目は、頼める参加者（手伝いを受け取れる、`peer_message` を知らせたアプリ）から 1 人選ぶ。選んで「依頼する」を押すまで何も送らない |
| 依頼への答え | 手伝う人 | 画面の中央の問いかけ（モーダル）。「手伝う」と「今はできない」。答えるまで何も変わらない。「手伝う」を押すとすぐ閉じ、手伝いが始まると帯が出る |
| 手伝いが続いていること・停止 | ルーム | ミキサーの上の帯。手伝われる側は、頼んで返事を待っている間は「◯◯さんの返事を待っています」[取り消す]（2 分たつと取り下げ、「返事がありませんでした」と出す）、手伝いが始まれば「◯◯さんが音声の設定を手伝っています」[停止]、手伝う側は「◯◯さんの設定を手伝っています」[手伝いをやめる]。手伝いのあいだ出続ける。手伝われる側の帯には「何もしないと、あと 27:12 で手伝いが終わります」と [記録を見る] も出る。残り時間は読み上げの対象にしない |
| 手伝いの記録 | 手伝われる人 | 別の小さなウィンドウ（`HelpLogScreen`、URL のハッシュ `#/help-log`）。手伝いの開始・設定の変更・ミュートと解除を時刻つきで並べ、残り時間と [手伝いを停止] を出す。メインの画面を押し出さずに記録を見られるように分けてある。手伝う人の窓には出さない（手伝う人は記録を読めない） |
| 手伝いが終わったあと | 手伝われる人 | 変わった設定があれば、帯に「◯◯さんが変えたのは 入力デバイス・バッファ です」と [元に戻す] [閉じる]（記録のウィンドウでも同じ）。何も変わっていなければ何も出さない。戻せなかった設定は名前を挙げて知らせ、[元に戻す] を残す。時間切れで終わったときは、その旨を一度きりの知らせに出す |
| 手伝う人の窓 | 手伝う人 | 別のウィンドウ（`HelperScreen`）。上に「◯◯さんの jamuru の設定を一緒に見ています。ここで変えたことは、◯◯さんの jamuru に反映されます。」の帯、その下に手伝いが終わるまでの残り時間の行、その下に手伝われる人のアプリの画面（`MainScreen`）。窓を閉じると手伝いが止まる |
| 相手の音声の設定 | 手伝う人 | 窓のヘッダーの設定ボタンで右から出る側面パネル（タイトル「◯◯さんの音声の設定」）。中身は設定ウィンドウと同じ Devices タブ。選ぶとその 1 件が相手のアプリに適用され、返った設定が出る。適用できなかったときは、その理由をパネルに出す |
| 相手の動き | 両方 | 手伝っているあいだ、相手のポインタが重なっている部分とフォーカスしている部分に枠（`PresenceMarks`）を出す。枠は押せる・フォーカスできる要素にだけ付き、相手の名前の小さな札と、ポインタの位置の点が付く（クリックは通す）。手伝う人の窓には、帯の下に「あなたの動きは◯◯さんにも見えています」を出し、相手の画面に届いたときだけ点を点ける |
| 手伝う人が開いている設定 | 手伝われる人 | 手伝う人が設定のパネルを開いているあいだ、同じ形の側面パネル（`SettingsHelpMirror`。タイトル「◯◯さんが開いている音声の設定」）を、画面の操作を止めない形（背景を暗くせず、Escape で閉じない）で出す。手伝われる人は閉じられ、手伝う人が開き直すまで出ない |
| 一度きりの知らせ（断られた・相手が止めた・相手が退出した） | ルーム | 帯の位置にトーストで数秒 |
| 変更の記録 | ルーム | チャットの system 行（下記） |

手伝う人の窓に**出さない**もの: 退室・チャットの送信欄・手伝いを頼む入口。相手のアプリはこれらを断るが、押して断られる操作は見せない。
相手の接続先は、窓は読みに行かない。

---

## 部品

| 部品 | 種類 | 役割 |
|------|------|------|
| `SettingsHelpAsk` | Pure | 依頼のダイアログ。中身の説明（3 行）→ 相手選び（ラジオの並び。1 人だけなら選ばれた状態）→「依頼する」。Escape と「やめる」で閉じる。外側のクリックでは閉じない。閉じるとフォーカスを元の場所に戻す |
| `SettingsHelpQuestion` | Pure | 問いかけ。`message`・`detail` と「手伝う」「今はできない」のボタン。最初のフォーカスは「今はできない」、Escape も今はできない。外側のクリックでは閉じない（うっかり答えさせない）。問いかけが出てから `ALLOW_DELAY_MS`（0.5 秒）は「手伝う」を受け付けない（`aria-disabled`。連打やひとつ前へのクリックで答えさせない）。閉じるとフォーカスを元の場所に戻す。別の問いかけは別のマウントにする: `useSettingsHelp` は依頼ごとに `key`（依頼ごとの通し番号）を付けて出ている間だけ描くので、最初の描画から「手伝う」を受け付けない |
| `SettingsHelpBar` | Pure | 帯。`message`・操作ボタンの並び |
| `SettingsHelpPanel` | Pure | 相手の設定。`useAudioSettingsTab` が作った Devices タブの props をそのまま描く。`waiting`（変更が相手のアプリへ向かっている間）は選べない |
| `useSettingsHelp` | Adapter（hook） | バックエンドが送るイベント（`session:settings-help`）から状態を作り、上の部品と入口の判定（`canAsk`・`openAsk`）を返す。手伝う人が「手伝う」と答えたあと、手伝われる側のイベント `taken` を受けて `settings_help_begin` を呼び、手伝いを始める |
| `SettingsHelpLog` | Pure | 手伝いの記録。見出し（誰が手伝っているか・終わったか）・残り時間・手伝いが終わったあとの知らせ（`after`）・時刻つきの行・[手伝いを停止] |
| `HelpLogScreen` | Adapter（画面） | 手伝いの記録のウィンドウ。`help_record_get` で記録を読み、イベント `help:log` で行を足し、[元に戻す]（`help_undo`）を呼ぶ。止める操作は、接続を持つメインの画面へ `help:stop-requested` で頼む |
| `useHelpRemaining` | Adapter（hook） | 残り時間。イベント `help:remaining` と記録の読み取りで合わせ、間は 1 秒ごとに数え下げる。`formatRemaining` は `m:ss`（秒は切り上げ） |
| `HelperScreen` | Adapter（画面） | 手伝う人の窓。`MainScreen`（`helper` を渡す）と設定パネルを、手伝われる人のアプリに繋いで描く。相手の設定は `settings_get`・`settings_change` とイベント `audio:config-changed`（番号の新しいほうを採る）で追う |
| `PresenceMarks` | Pure | 相手の動きの枠。`marks`（部分の画面上の位置と大きさ、ポインタの位置）と `name` を受けて、画面に固定した枠を描く。クリックを通し、スクリーンリーダーには読ませない（`aria-hidden`）。REQ-RMT-033 のデバッグの枠（Alt+Shift）とは別の見た目（赤い枠）で、デバッグ版の判定の部品（`lib/domTarget.ts`）だけを共有する |
| `useHelpPresence` | Adapter（hook） | `role`（`helper` / `helped`）に応じて、自分の動き（`lib/presence.ts` の `watchPresence`。1 秒に 10 回まで、同じ絵は送らない）を相手へ送り、相手の動きを枠（`marks`）にする。手伝う人は `help_presence` の答えで届いたかを知り、手伝われる人は `help_presence_publish` で公開し、手伝う人の窓がイベント `help:presence` として受ける |
| `SettingsHelpMirror` | Pure | 手伝われる人の画面の、手伝う人が開いている設定の側面パネル（`SidePanel` の `modal=false`）。`useAudioSettingsPanel` が作った中身を `HelperScreen` と共有する |
| `useAudioSettingsTab` | 共通（hook） | 音声の設定から Devices タブの props を作る。設定ウィンドウと手伝う人のパネルが同じ選択肢を出すための 1 か所 |

窓は、画面から見たバックエンドの入口（`lib/backend.ts`）を `lib/helperBackend.ts` に差し替えて動く。呼びは相手のアプリへ（`help_call`）、
イベントは相手のアプリのものが届く。窓自身のものは、言語・ログ・大きさ（`config_get_language` など）だけ。相手のアプリが呼びを断ると
（`denied`）、呼びの失敗として届く。

`data-testid`: `settings-help-ask-open`（入口）・`settings-help-ask` / `settings-help-ask-next` / `settings-help-ask-choice` / `settings-help-ask-send` / `settings-help-ask-cancel`（依頼）・`settings-help-question` / `settings-help-allow` / `settings-help-decline`（問いかけ）・
`settings-help-bar` / `settings-help-stop-helped` / `settings-help-stop-helper` / `settings-help-cancel`（帯）・
`settings-help-window` / `settings-help-window-banner` / `settings-help-window-waiting`（手伝う人の窓）・
`settings-help-panel` / `settings-help-panel-status`（パネル）・`settings-help-window-presence`（動きが見えている旨と、届いたかの点。`data-delivered`）・`presence-marks` / `presence-mark-hover` / `presence-mark-focus`（枠）・
`settings-help-status` / `settings-help-log-open` / `settings-help-undo` / `settings-help-report-dismiss`（帯）・`settings-help-window-remaining`（手伝う人の窓）・
`settings-help-log` / `settings-help-log-heading` / `settings-help-log-remaining` / `settings-help-log-lines` / `settings-help-log-stop` / `settings-help-log-undo` / `settings-help-log-undone`（記録のウィンドウ）。

---

## 文言

| キー | 日本語 | English |
|------|--------|---------|
| `settingsHelp.ask.button` | 設定依頼をする | Ask for settings help |
| `settingsHelp.ask.title` | 設定依頼をしますか？ | Ask for help with settings? |
| `settingsHelp.ask.what` / `.limits` / `.control` | 選んだ人が、jamuru の設定を一緒に見て、変えてくれます。/ パソコンの画面や jamuru 以外のものは見えません。/ いつでもあなたが止められます。 | Someone you choose will look at jamuru's settings with you and can change them. / They can't see the rest of your computer. / You can stop it at any time. |
| `settingsHelp.ask.chooseTitle` | 誰に頼みますか？ | Who do you want to ask? |
| `settingsHelp.request.message` / `.detail` | {{name}}さんから設定依頼が届きました / {{name}}さんの jamuru の設定を、一緒に見て手伝います。 | {{name}} has asked for help with settings / You'll look at {{name}}'s jamuru settings together. |
| `settingsHelp.request.allow` / `.decline` | 手伝う / 今はできない | Help / Not now |
| `settingsHelp.ask.declined` / `.busy` / `.noAnswer` / `.withdrawn` | {{name}}さんは今回は手伝えませんでした / いま別の人を手伝っています / 返事がありませんでした / 設定依頼を取り消しました | can't help right now / is helping someone else right now / no answer / cancelled the request |
| `settingsHelp.helped.bar` / `.stop` | {{name}}さんが音声の設定を手伝っています / 停止 | {{name}} is helping with your audio settings / Stop |
| `settingsHelp.helped.remaining` / `settingsHelp.helper.remaining` | 動きがなければ、あと {{time}} で手伝いが自動で終わります | If nothing is done, the help ends by itself in {{time}} |
| `settingsHelp.helped.log` | 記録を見る | View log |
| `settingsHelp.log.started` / `.settingChanged` / `.muted` / `.unmuted` | {{name}}さんが手伝いを始めました / {{name}}さんが{{setting}}設定を変えました / {{name}}さんがミュートにしました / {{name}}さんがミュートを解除しました | {{name}} started helping / {{name}} changed the {{setting}} setting / {{name}} muted the microphone / {{name}} unmuted the microphone |
| `settingsHelp.report.changed` / `.undo` / `.dismiss` | {{name}}さんが変えたのは {{settings}} です（設定の名前は「・」でつなぐ） / 元に戻す / 閉じる | {{name}} changed: {{settings}} / Put back / Close |
| `settingsHelp.report.restored` / `.notRestored` | 手伝いの前の設定に戻しました / {{settings}}は戻せませんでした。設定の画面で選び直してください | Put the settings back as they were before the help / Couldn't put back {{settings}}. Choose it again in the settings |
| `settingsHelp.ended.stopped` / `.timedOut` | {{name}}さんが手伝いを終えました / しばらく動きがなかったので、{{name}}さんの手伝いを終えました | {{name}} ended the help / Nothing was done for a while, so the help from {{name}} ended |
| `settingsHelp.window.banner` | {{name}}さんの jamuru の設定を一緒に見ています。ここで変えたことは、{{name}}さんの jamuru に反映されます。 | You are looking at {{name}}'s jamuru settings together. What you change here changes theirs. |
| `settingsHelp.window.presence` | あなたの動きは{{name}}さんにも見えています | {{name}} can also see where you point and focus |
| `settingsHelp.helped.panelTitle` | {{name}}さんが開いている音声の設定 | Audio settings {{name}} has open |
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
