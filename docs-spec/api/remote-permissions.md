<!-- このドキュメントは src-tauri/src/rpc/spec.rs の表から作る。直すときは表を直して再生成する -->

# 手伝いで相手に何をされうるか

設定の手伝い（[ADR-044](../adr/ADR-044-portals-and-permissions.md)）で、手伝う人が手伝われる人のアプリに対してできる操作の一覧。
判定は手伝われる人のアプリが行い、この表に無い操作は呼べない。

## できる操作

| 操作 | 内容 |
|------|------|
| `greet` | 動作確認用のあいさつ |
| `session_get` | 接続・入室の状態を読む |
| `signaling_get_chat_messages` | チャットの履歴を読む |
| `settings_get` | 音声の設定を読む |
| `settings_change` | 音声の設定を変える |
| `streaming_status` | 音声の状態（レベル・遅延・統計）を読む |
| `streaming_set_mute` | 自分のマイクをミュートする・戻す |
| `streaming_get_mute` | 自分のマイクのミュートの状態を読む |
| `streaming_set_monitoring` | 自分の音のモニターを切り替える |
| `streaming_get_input_level` | 入力のレベルを読む |
| `streaming_set_peer_volume` | 相手ごとの音量を変える |
| `streaming_get_peer_volume` | 相手ごとの音量を読む |
| `streaming_set_master_volume` | 全体の音量を変える |
| `streaming_get_master_volume` | 全体の音量を読む |
| `streaming_set_peer_pan` | 相手ごとの左右の位置を変える |
| `streaming_get_peer_pan` | 相手ごとの左右の位置を読む |
| `streaming_set_local_volume` | 自分の音量を変える |
| `streaming_get_local_volume` | 自分の音量を読む |
| `streaming_set_local_pan` | 自分の左右の位置を変える |
| `streaming_get_local_pan` | 自分の左右の位置を読む |
| `config_list_presets` | プリセットの一覧を読む |
| `config_get_preset` | プリセットの内容を読む |
| `config_get_peer_name` | 自分の表示名を読む |
| `config_set_peer_name` | 自分の表示名を変える |
| `config_get_sample_rate` | サンプルレートの設定を読む |
| `config_get_transmit_channels` | 送信チャンネル数の設定を読む |
| `config_get_language` | 表示の言語を読む |
| `config_set_language` | 表示の言語を変える |
| `diagnostics_run_complete` | 診断を全部走らせる |
| `diagnostics_run_network` | ネットワークの診断を走らせる |
| `diagnostics_run_audio` | 音声の診断を走らせる |
| `diagnostics_run_cpu` | CPU の診断を走らせる |
| `diagnostics_get_recommended_preset` | 診断からおすすめのプリセットを読む |
| `diagnostics_check_zero_latency` | ゼロレイテンシーモードが使えるかを調べる |
| `usage_preview` | 利用状況として送る内容の見本を読む |

## できない操作

チャットの送信・リアクション、退室とルームの移動、接続先と利用状況の送信の設定、入ったことのあるルームの履歴、
ウィンドウの操作、ほかの人への手伝いの申し出は、手伝う人にはできない。

| 操作 | 内容 |
|------|------|
| `session_connect` | サーバーへの接続を最初からやり直す |
| `session_create` | ルームを作って入る |
| `session_join` | ルームに参加する |
| `session_leave` | ルームから退室する |
| `session_reconnect` | 切れたシグナリングを繋ぎ直して、ルームに入り直す |
| `signaling_send_chat` | チャットを送る |
| `signaling_add_reaction` | チャットにリアクションを付ける |
| `signaling_remove_reaction` | チャットのリアクションを外す |
| `signaling_toggle_reaction` | チャットのリアクションを付け外しする |
| `settings_help_request` | 設定の手伝いを申し出る |
| `settings_help_answer` | 設定の手伝いの申し出に答える |
| `settings_help_propose` | 設定の変更を申請する |
| `settings_help_decide` | 設定の変更の申請に答える |
| `settings_help_stop` | 設定の手伝いを止める |
| `streaming_reconnect` | 音声の接続を張り直す |
| `config_load` | 設定ファイルの全体を読む |
| `config_set_usage_reporting` | 利用状況の送信をオン・オフする |
| `config_get_server_url` | 接続先のサーバーを読む |
| `config_set_server_url` | 接続先のサーバーを変える |
| `config_get_effective_server_url` | 実際に使う接続先を読む |
| `config_get_connection_history` | 入ったことのあるルームの履歴を読む |
| `config_add_connection_history` | 入ったことのあるルームの履歴に足す |
| `config_remove_connection_history` | 入ったことのあるルームの履歴から消す |
| `config_clear_connection_history` | 入ったことのあるルームの履歴を全部消す |
| `config_update_connection_history_label` | 入ったことのあるルームの履歴の名前を変える |
| `window_open_settings` | 設定ウィンドウを開く |
| `window_close_settings` | 設定ウィンドウを閉じる |
| `window_toggle_chat` | チャットウィンドウを開閉する |
| `window_show_chat` | チャットウィンドウを出す |
| `window_hide_chat` | チャットウィンドウを隠す |
| `window_session_connected` | セッション中のウィンドウ構成に切り替える |
| `window_session_disconnected` | セッション前のウィンドウ構成に戻す |
| `window_is_in_session` | セッション中かを読む |
| `window_focus` | ウィンドウを前面に出す |
| `window_resize_main` | メインウィンドウの大きさを変える |
| `log_frontend` | 画面のログを診断ログに書く |
| `log_open_dir` | 診断ログのフォルダを開く |
