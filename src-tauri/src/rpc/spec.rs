//! The permission table: which portal may call which method (ADR-044).
//!
//! This is the one place that says what each way into the app may do. A row is
//! a method, a column is a portal. A method that is not in the table cannot be
//! called from any portal, and the list Tauri routes commands from
//! ([`invoke_handler`]) is built from the same rows, so a command cannot be
//! registered without deciding who may call it.
//!
//! Change a row's access and the generated public document
//! (`docs-spec/api/remote-permissions.md`) stops matching until it is
//! regenerated, which is what keeps a change of range visible in review.

/// A way into the app (ADR-044 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Portal {
    /// The app's own webview (IPC).
    Screen,
    /// The E2E control channel on loopback (`e2e-control` builds only).
    Loopback,
    /// Remote debugging through the relay (beta builds only).
    Debug,
    /// Someone helping with this app's settings, through the relay.
    Help,
}

/// The set of portals a method may be called from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access(u8);

impl Access {
    pub const SCREEN: Access = Access(1);
    pub const LOOPBACK: Access = Access(2);
    pub const DEBUG: Access = Access(4);
    pub const HELP: Access = Access(8);

    /// Every portal.
    pub const ALL: Access = Access(15);
    /// Every portal but settings help: the method would act outside the range
    /// a person who is helping has been trusted with (ADR-044 §3).
    pub const NO_HELP: Access = Access(7);
    /// Only the app's own webview.
    pub const SCREEN_ONLY: Access = Access(1);
    /// The two portals that exist to test and debug the app.
    pub const TOOLS: Access = Access(6);

    pub const fn allows(self, portal: Portal) -> bool {
        let bit = match portal {
            Portal::Screen => 1,
            Portal::Loopback => 2,
            Portal::Debug => 4,
            Portal::Help => 8,
        };
        self.0 & bit != 0
    }
}

/// How a method is carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A Tauri command, called through the webview's IPC exactly as the UI
    /// calls it, so the IPC's own checks stay in the path.
    App,
    /// Implemented in Rust, with no webview call.
    Native,
}

/// One row of the table.
#[derive(Debug, Clone, Copy)]
pub struct Method {
    pub name: &'static str,
    pub access: Access,
    pub kind: Kind,
    /// What it does, in the words of the public document.
    pub summary: &'static str,
}

/// The name of a command from its path: the last segment.
macro_rules! last_ident {
    ($last:ident) => {
        stringify!($last)
    };
    ($first:ident :: $($rest:ident)::+) => {
        last_ident!($($rest)::+)
    };
}

/// Declares the app's commands: builds [`APP_METHODS`] and [`invoke_handler`]
/// from one list, so the two cannot disagree.
macro_rules! app_commands {
    ($( [$access:expr] $summary:literal $($seg:ident)::+ ; )*) => {
        /// The app's Tauri commands with who may call them.
        pub const APP_METHODS: &[Method] = &[
            $( Method {
                name: last_ident!($($seg)::+),
                access: $access,
                kind: Kind::App,
                summary: $summary,
            } ),*
        ];

        /// What Tauri routes an IPC call through.
        pub fn invoke_handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
            tauri::generate_handler![ $( $($seg)::+ ),* ]
        }
    };
}

app_commands! {
    [Access::ALL] "動作確認用のあいさつ" crate::greet;

    // Reaching the signaling server and rooms. A helper works inside the
    // room the helped person is already in: they may not move or speak for
    // them (ADR-044 §3).
    [Access::NO_HELP] "シグナリングサーバーに接続する" crate::signaling::signaling_connect;
    [Access::NO_HELP] "シグナリングサーバーから切断する" crate::signaling::signaling_disconnect;
    [Access::NO_HELP] "ルームの一覧を取る" crate::signaling::signaling_list_rooms;
    [Access::NO_HELP] "ルームに参加する" crate::signaling::signaling_join_room;
    [Access::NO_HELP] "ルームから退室する" crate::signaling::signaling_leave_room;
    [Access::NO_HELP] "ルームを作る" crate::signaling::signaling_create_room;
    [Access::NO_HELP] "チャットを送る" crate::signaling::signaling_send_chat;
    [Access::ALL] "チャットの履歴を読む" crate::signaling::signaling_get_chat_messages;
    [Access::NO_HELP] "チャットにリアクションを付ける" crate::signaling::signaling_add_reaction;
    [Access::NO_HELP] "チャットのリアクションを外す" crate::signaling::signaling_remove_reaction;
    [Access::NO_HELP] "チャットのリアクションを付け外しする" crate::signaling::signaling_toggle_reaction;
    [Access::NO_HELP] "自分の接続先の候補をルームに知らせる" crate::signaling::signaling_publish_local_candidates;
    // A read that takes the events off the queue: a second reader would
    // steal them from the person's own screen.
    [Access::NO_HELP] "ルームの出来事を受け取る" crate::signaling::signaling_poll_events;
    [Access::NO_HELP] "設定の手伝いを申し出る" crate::signaling::settings_help_request;
    [Access::NO_HELP] "設定の手伝いの申し出に答える" crate::signaling::settings_help_answer;
    [Access::NO_HELP] "設定の変更を申請する" crate::signaling::settings_help_propose;
    [Access::NO_HELP] "設定の変更の申請に答える" crate::signaling::settings_help_decide;
    [Access::NO_HELP] "設定の手伝いを止める" crate::signaling::settings_help_stop;

    [Access::ALL] "音声の設定を読む" crate::settings::settings_get;
    [Access::ALL] "音声の設定を変える" crate::settings::settings_change;

    [Access::NO_HELP] "音声の送受信の準備をする" crate::streaming::streaming_prepare;
    [Access::NO_HELP] "音声の送受信を始める" crate::streaming::streaming_start;
    [Access::NO_HELP] "音声の送受信を止める" crate::streaming::streaming_stop;
    [Access::ALL] "音声の状態（レベル・遅延・統計）を読む" crate::streaming::streaming_status;
    [Access::NO_HELP] "音声の接続を張り直す" crate::streaming::streaming_reconnect;
    [Access::ALL] "自分のマイクをミュートする・戻す" crate::streaming::streaming_set_mute;
    [Access::ALL] "自分のマイクのミュートの状態を読む" crate::streaming::streaming_get_mute;
    [Access::ALL] "自分の音のモニターを切り替える" crate::streaming::streaming_set_monitoring;
    [Access::ALL] "入力のレベルを読む" crate::streaming::streaming_get_input_level;
    [Access::ALL] "相手ごとの音量を変える" crate::streaming::streaming_set_peer_volume;
    [Access::ALL] "相手ごとの音量を読む" crate::streaming::streaming_get_peer_volume;
    [Access::ALL] "全体の音量を変える" crate::streaming::streaming_set_master_volume;
    [Access::ALL] "全体の音量を読む" crate::streaming::streaming_get_master_volume;
    [Access::ALL] "相手ごとの左右の位置を変える" crate::streaming::streaming_set_peer_pan;
    [Access::ALL] "相手ごとの左右の位置を読む" crate::streaming::streaming_get_peer_pan;
    [Access::ALL] "自分の音量を変える" crate::streaming::streaming_set_local_volume;
    [Access::ALL] "自分の音量を読む" crate::streaming::streaming_get_local_volume;
    [Access::ALL] "自分の左右の位置を変える" crate::streaming::streaming_set_local_pan;
    [Access::ALL] "自分の左右の位置を読む" crate::streaming::streaming_get_local_pan;

    // The whole config includes the server address and the rooms visited.
    [Access::NO_HELP] "設定ファイルの全体を読む" crate::config::config_load;
    // Consent to send usage data is the person's own.
    [Access::NO_HELP] "利用状況の送信をオン・オフする" crate::config::config_set_usage_reporting;
    // Where the app connects is outside what a helper is trusted with.
    [Access::NO_HELP] "接続先のサーバーを読む" crate::config::config_get_server_url;
    [Access::NO_HELP] "接続先のサーバーを変える" crate::config::config_set_server_url;
    [Access::NO_HELP] "実際に使う接続先を読む" crate::config::config_get_effective_server_url;
    [Access::ALL] "プリセットの一覧を読む" crate::config::config_list_presets;
    [Access::ALL] "プリセットの内容を読む" crate::config::config_get_preset;
    // The rooms a person has been in is personal.
    [Access::NO_HELP] "入ったことのあるルームの履歴を読む" crate::config::config_get_connection_history;
    [Access::NO_HELP] "入ったことのあるルームの履歴に足す" crate::config::config_add_connection_history;
    [Access::NO_HELP] "入ったことのあるルームの履歴から消す" crate::config::config_remove_connection_history;
    [Access::NO_HELP] "入ったことのあるルームの履歴を全部消す" crate::config::config_clear_connection_history;
    [Access::NO_HELP] "入ったことのあるルームの履歴の名前を変える" crate::config::config_update_connection_history_label;
    [Access::ALL] "自分の表示名を読む" crate::config::config_get_peer_name;
    [Access::ALL] "自分の表示名を変える" crate::config::config_set_peer_name;
    [Access::ALL] "サンプルレートの設定を読む" crate::config::config_get_sample_rate;
    [Access::ALL] "送信チャンネル数の設定を読む" crate::config::config_get_transmit_channels;
    [Access::ALL] "表示の言語を読む" crate::config::config_get_language;
    [Access::ALL] "表示の言語を変える" crate::config::config_set_language;

    [Access::ALL] "診断を全部走らせる" crate::diagnostics::diagnostics_run_complete;
    [Access::ALL] "ネットワークの診断を走らせる" crate::diagnostics::diagnostics_run_network;
    [Access::ALL] "音声の診断を走らせる" crate::diagnostics::diagnostics_run_audio;
    [Access::ALL] "CPU の診断を走らせる" crate::diagnostics::diagnostics_run_cpu;
    [Access::ALL] "診断からおすすめのプリセットを読む" crate::diagnostics::diagnostics_get_recommended_preset;
    [Access::ALL] "ゼロレイテンシーモードが使えるかを調べる" crate::diagnostics::diagnostics_check_zero_latency;

    // The person's own windows: a helper's screen has its own.
    [Access::NO_HELP] "設定ウィンドウを開く" crate::windows::window_open_settings;
    [Access::NO_HELP] "設定ウィンドウを閉じる" crate::windows::window_close_settings;
    [Access::NO_HELP] "チャットウィンドウを開閉する" crate::windows::window_toggle_chat;
    [Access::NO_HELP] "チャットウィンドウを出す" crate::windows::window_show_chat;
    [Access::NO_HELP] "チャットウィンドウを隠す" crate::windows::window_hide_chat;
    [Access::NO_HELP] "セッション中のウィンドウ構成に切り替える" crate::windows::window_session_connected;
    [Access::NO_HELP] "セッション前のウィンドウ構成に戻す" crate::windows::window_session_disconnected;
    [Access::NO_HELP] "セッション中かを読む" crate::windows::window_is_in_session;
    [Access::NO_HELP] "ウィンドウを前面に出す" crate::windows::window_focus;
    [Access::NO_HELP] "メインウィンドウの大きさを変える" crate::windows::window_resize_main;
    [Access::NO_HELP] "画面のログを診断ログに書く" crate::logging::log_frontend;
    [Access::NO_HELP] "診断ログのフォルダを開く" crate::logging::log_open_dir;
    [Access::ALL] "利用状況として送る内容の見本を読む" crate::usage::usage_preview;

    // The screen's own answer to a call a portal started (see `webview`).
    [Access::SCREEN_ONLY] "遠隔の呼び出しの結果を返す" crate::rpc::rpc_settle;
}

/// Every method in the table, app commands first.
pub fn all_methods() -> impl Iterator<Item = &'static Method> {
    APP_METHODS.iter().chain(super::native_methods().iter())
}

/// The row for `name`, if there is one.
pub fn find(name: &str) -> Option<&'static Method> {
    all_methods().find(|method| method.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The public document lists what a person who is helping may do, from
    /// this table. Regenerate with `JAMJAM_UPDATE_REMOTE_PERMISSIONS=1 cargo test -p jamjam-app rpc::spec`.
    const DOC_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs-spec/api/remote-permissions.md"
    );

    const HEAD: &str = "<!-- このドキュメントは src-tauri/src/rpc/spec.rs の表から作る。直すときは表を直して再生成する -->

# 手伝いで相手に何をされうるか

設定の手伝い（[ADR-044](../adr/ADR-044-portals-and-permissions.md)）で、手伝う人が手伝われる人のアプリに対してできる操作の一覧。
判定は手伝われる人のアプリが行い、この表に無い操作は呼べない。

## できる操作

";
    const MIDDLE: &str = "
## できない操作

チャットの送信・リアクション、退室とルームの移動、接続先と利用状況の送信の設定、入ったことのあるルームの履歴、
ウィンドウの操作、ほかの人への手伝いの申し出は、手伝う人にはできない。

";

    fn help_permissions_doc() -> String {
        let mut allowed = String::new();
        let mut refused = String::new();
        for method in APP_METHODS.iter().filter(|m| m.name != "rpc_settle") {
            let line = format!("| `{}` | {} |\n", method.name, method.summary);
            if method.access.allows(Portal::Help) {
                allowed.push_str(&line);
            } else {
                refused.push_str(&line);
            }
        }
        format!(
            "{HEAD}| 操作 | 内容 |\n|------|------|\n{allowed}{MIDDLE}| 操作 | 内容 |\n|------|------|\n{refused}"
        )
    }

    /// Verifies: REQ-RMT-022
    #[test]
    fn a_method_has_one_row() {
        let mut names: Vec<&str> = all_methods().map(|m| m.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            total,
            "a method name appears twice in the table"
        );
    }

    /// The methods that read or drive the screen and the app's internals are
    /// for testing and debugging only: neither the person's own screen nor a
    /// person helping may call them.
    ///
    /// Verifies: REQ-RMT-025
    #[test]
    fn a_tool_method_is_never_open_to_the_screen_or_to_a_helper() {
        for method in all_methods().filter(|m| m.kind == Kind::Native) {
            assert!(
                method.name.starts_with("ui.") || method.name.starts_with("debug."),
                "{} is native but is not a ui.* or debug.* method",
                method.name
            );
            assert!(
                !method.access.allows(Portal::Help),
                "{} is open to a helper",
                method.name
            );
            assert!(
                !method.access.allows(Portal::Screen),
                "{} is open to the screen, which has no use for it",
                method.name
            );
        }
    }

    /// What a helper is not trusted with (ADR-044 §3): speaking for the
    /// person, leaving or moving them, and changing where the app connects or
    /// what it sends.
    ///
    /// Verifies: REQ-RMT-025
    #[test]
    fn a_helper_may_not_speak_move_or_redirect_the_person() {
        for name in [
            "signaling_send_chat",
            "signaling_add_reaction",
            "signaling_remove_reaction",
            "signaling_toggle_reaction",
            "signaling_leave_room",
            "signaling_disconnect",
            "signaling_join_room",
            "signaling_create_room",
            "streaming_stop",
            "config_set_server_url",
            "config_set_usage_reporting",
            "config_get_connection_history",
            "settings_help_request",
            "log_open_dir",
            "rpc_settle",
        ] {
            let method = find(name).unwrap_or_else(|| panic!("{} is not in the table", name));
            assert!(
                !method.access.allows(Portal::Help),
                "{} is open to a helper",
                name
            );
        }
    }

    /// Muting and the audio settings are what a helper is there to do.
    ///
    /// Verifies: REQ-RMT-025
    #[test]
    fn a_helper_may_mute_and_change_the_audio_settings() {
        for name in [
            "streaming_set_mute",
            "settings_get",
            "settings_change",
            "streaming_status",
        ] {
            assert!(
                find(name).unwrap().access.allows(Portal::Help),
                "{} is not open to a helper",
                name
            );
        }
    }

    /// Only the app's own screen reports how a call settled.
    ///
    /// Verifies: REQ-RMT-022
    #[test]
    fn only_the_screen_may_report_how_a_call_settled() {
        let access = find("rpc_settle").unwrap().access;
        assert!(access.allows(Portal::Screen));
        for portal in [Portal::Loopback, Portal::Debug, Portal::Help] {
            assert!(!access.allows(portal), "{:?} may call rpc_settle", portal);
        }
    }

    /// Verifies: REQ-RMT-022
    #[test]
    fn the_public_document_lists_what_the_permission_table_lets_a_helper_do() {
        let generated = help_permissions_doc();
        if std::env::var_os("JAMJAM_UPDATE_REMOTE_PERMISSIONS").is_some() {
            std::fs::write(DOC_PATH, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(DOC_PATH).unwrap_or_default();
        assert_eq!(
            committed, generated,
            "docs-spec/api/remote-permissions.md is out of date; regenerate it with \
             JAMJAM_UPDATE_REMOTE_PERMISSIONS=1"
        );
    }
}
