//! The native menu bar (ADR-081).
//!
//! What the menu holds, in which language, and which entries can be used right
//! now are all decided here by plain functions of plain values ([`layout`],
//! [`Item::enabled`], [`Lang::resolve`]), so the rules are tested without a
//! window. [`install`] and [`refresh`] only carry them onto the OS's menu.
//!
//! No entry has a keyboard shortcut of its own. The shortcuts the OS gives the
//! standard entries (copy, quit, ...) are the OS's; ours are decided together
//! with the app-wide shortcuts, not here.

use std::sync::Mutex;

use tauri::menu::{
    AboutMetadata, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID,
    WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Emitter, Listener, Manager, Wry};

use crate::config::ConfigState;
use crate::session::SessionState;
use crate::terms::TermsState;
use crate::windows;
use jamjam::network::room_session::Phase;

/// What the main window is told when an entry that works on its screen is
/// chosen. The payload is the entry's [`Item::action`].
pub const ACTION_EVENT: &str = "menu:action";

const GUIDE_URL: &str =
    "https://github.com/koedame/jamuru-client/blob/HEAD/docs/getting-started/quick-start.md";
const TROUBLESHOOTING_URL: &str =
    "https://github.com/koedame/jamuru-client/blob/HEAD/docs/getting-started/troubleshooting.md";

/// The language of the menu: the two the screen has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Ja,
    En,
}

impl Lang {
    /// The language the person chose, else the one the OS is set to, else
    /// English: the screen's order (`ui/src/i18n/config.ts`).
    pub fn resolve(chosen: Option<&str>, system: Option<&str>) -> Lang {
        match chosen.or(system) {
            Some(tag) if tag.to_ascii_lowercase().starts_with("ja") => Lang::Ja,
            _ => Lang::En,
        }
    }
}

/// What kind of window the menu is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// The join / room window.
    Main,
    Settings,
    /// A window someone helping works in, or the record of a help.
    Help,
}

impl WindowKind {
    pub fn of(label: &str) -> WindowKind {
        match label {
            windows::labels::SETTINGS => WindowKind::Settings,
            label if label.starts_with("help-") => WindowKind::Help,
            _ => WindowKind::Main,
        }
    }
}

/// What decides which entries can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    /// The window the menu is used from.
    pub window: WindowKind,
    /// The terms of use are agreed to (REQ-TRM-003).
    pub terms_accepted: bool,
    /// Connected to the server and in no room, so a room can be made or joined.
    pub can_enter_room: bool,
    /// The server offers a test room.
    pub test_room: bool,
    /// In a room.
    pub in_room: bool,
}

impl Context {
    fn from_app(app: &AppHandle, window: WindowKind) -> Context {
        let snapshot = app.try_state::<SessionState>().map(|s| s.snapshot());
        Context {
            window,
            terms_accepted: app
                .try_state::<TermsState>()
                .is_some_and(|terms| terms.is_accepted()),
            can_enter_room: snapshot.as_ref().is_some_and(|s| {
                s.connection_id.is_some()
                    && matches!(s.phase, Phase::ServerConnected | Phase::Error)
            }),
            test_room: snapshot
                .as_ref()
                .is_some_and(|s| s.test_room_invite_code.is_some()),
            in_room: snapshot
                .as_ref()
                .is_some_and(|s| s.phase == Phase::Connected),
        }
    }
}

/// An entry the app made; the OS's own entries are [`Native`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    CreateRoom,
    JoinRoom,
    TestRoom,
    CopyInviteCode,
    LeaveRoom,
    AskForHelp,
    Settings,
    /// Quit, where the OS has no entry that does it (Windows / Linux).
    Quit,
    Guide,
    Troubleshooting,
    Diagnostics,
    OpenLogFolder,
    ReportProblem,
    Terms,
    Licenses,
    Privacy,
    Announcements,
}

/// What choosing an entry does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The main window's screen does it (`ui/src/screens/MainScreen.tsx`).
    Screen(&'static str),
    /// Opens the settings window, on a tab.
    Settings(Option<&'static str>),
    Browser(&'static str),
    LogFolder,
    Exit,
}

impl Item {
    pub const ALL: [Item; 17] = [
        Item::CreateRoom,
        Item::JoinRoom,
        Item::TestRoom,
        Item::CopyInviteCode,
        Item::LeaveRoom,
        Item::AskForHelp,
        Item::Settings,
        Item::Quit,
        Item::Guide,
        Item::Troubleshooting,
        Item::Diagnostics,
        Item::OpenLogFolder,
        Item::ReportProblem,
        Item::Terms,
        Item::Licenses,
        Item::Privacy,
        Item::Announcements,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Item::CreateRoom => "menu.create_room",
            Item::JoinRoom => "menu.join_room",
            Item::TestRoom => "menu.test_room",
            Item::CopyInviteCode => "menu.copy_invite_code",
            Item::LeaveRoom => "menu.leave_room",
            Item::AskForHelp => "menu.ask_for_help",
            Item::Settings => "menu.settings",
            Item::Quit => "menu.quit",
            Item::Guide => "menu.guide",
            Item::Troubleshooting => "menu.troubleshooting",
            Item::Diagnostics => "menu.diagnostics",
            Item::OpenLogFolder => "menu.open_log_folder",
            Item::ReportProblem => "menu.report_problem",
            Item::Terms => "menu.terms",
            Item::Licenses => "menu.licenses",
            Item::Privacy => "menu.privacy",
            Item::Announcements => "menu.announcements",
        }
    }

    pub fn from_id(id: &str) -> Option<Item> {
        Item::ALL.into_iter().find(|item| item.id() == id)
    }

    pub fn route(self) -> Route {
        match self {
            Item::CreateRoom => Route::Screen("create_room"),
            Item::JoinRoom => Route::Screen("join_room"),
            Item::TestRoom => Route::Screen("test_room"),
            Item::CopyInviteCode => Route::Screen("copy_invite_code"),
            Item::LeaveRoom => Route::Screen("leave_room"),
            Item::AskForHelp => Route::Screen("ask_for_help"),
            Item::Settings => Route::Settings(None),
            Item::Quit => Route::Exit,
            Item::Guide => Route::Browser(GUIDE_URL),
            Item::Troubleshooting => Route::Browser(TROUBLESHOOTING_URL),
            Item::Diagnostics | Item::ReportProblem => Route::Settings(Some("diagnostics")),
            Item::OpenLogFolder => Route::LogFolder,
            Item::Terms | Item::Licenses => Route::Settings(Some("about")),
            Item::Privacy => Route::Browser(crate::terms::PRIVACY_URL),
            Item::Announcements => Route::Browser(crate::terms::ANNOUNCEMENTS_URL),
        }
    }

    /// Whether the entry can be used now. A disabled entry stays in the menu
    /// so the menu is the same wherever it is opened.
    pub fn enabled(self, context: &Context) -> bool {
        let main = context.window == WindowKind::Main;
        // The person's own settings window: a helper's window has the helped
        // app's screen instead.
        let own_settings = matches!(context.window, WindowKind::Main | WindowKind::Settings)
            && context.terms_accepted;
        match self {
            Item::CreateRoom | Item::JoinRoom => {
                main && context.terms_accepted && context.can_enter_room
            }
            Item::TestRoom => {
                main && context.terms_accepted && context.can_enter_room && context.test_room
            }
            Item::CopyInviteCode | Item::LeaveRoom | Item::AskForHelp => main && context.in_room,
            Item::Settings
            | Item::Diagnostics
            | Item::ReportProblem
            | Item::Terms
            | Item::Licenses => own_settings,
            Item::Quit
            | Item::Guide
            | Item::Troubleshooting
            | Item::OpenLogFolder
            | Item::Privacy
            | Item::Announcements => true,
        }
    }

    pub fn text(self, lang: Lang) -> &'static str {
        let (ja, en) = match self {
            Item::CreateRoom => ("ルームを作成", "Create Room"),
            Item::JoinRoom => ("招待コードで参加…", "Join with Invite Code…"),
            Item::TestRoom => ("テストルームに入る", "Enter Test Room"),
            Item::CopyInviteCode => ("招待コードをコピー", "Copy Invite Code"),
            Item::LeaveRoom => ("退室…", "Leave Room…"),
            Item::AskForHelp => ("設定依頼をする…", "Ask for Help with Settings…"),
            Item::Settings => ("設定…", "Settings…"),
            Item::Quit => ("終了", "Exit"),
            Item::Guide => ("jamuru の使い方", "jamuru Guide"),
            Item::Troubleshooting => ("うまくいかないとき", "Troubleshooting"),
            Item::Diagnostics => ("システム診断…", "System Diagnostics…"),
            Item::OpenLogFolder => ("ログフォルダを開く", "Open Log Folder"),
            Item::ReportProblem => ("問題を報告…", "Report a Problem…"),
            Item::Terms => ("利用規約", "Terms of Use"),
            Item::Licenses => ("ライセンス", "Licenses"),
            Item::Privacy => ("プライバシーとセキュリティ", "Privacy and Security"),
            Item::Announcements => ("お知らせ", "Announcements"),
        };
        match lang {
            Lang::Ja => ja,
            Lang::En => en,
        }
    }
}

/// An entry the OS provides and carries out itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Native {
    About,
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    CloseWindow,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Fullscreen,
    Minimize,
    Zoom,
    BringAllToFront,
}

impl Native {
    pub fn text(self, lang: Lang) -> &'static str {
        let (ja, en) = match self {
            Native::About => ("jamuru について", "About jamuru"),
            Native::Services => ("サービス", "Services"),
            Native::Hide => ("jamuru を隠す", "Hide jamuru"),
            Native::HideOthers => ("ほかを隠す", "Hide Others"),
            Native::ShowAll => ("すべてを表示", "Show All"),
            Native::Quit => ("jamuru を終了", "Quit jamuru"),
            Native::CloseWindow => ("ウィンドウを閉じる", "Close Window"),
            Native::Undo => ("取り消す", "Undo"),
            Native::Redo => ("やり直す", "Redo"),
            Native::Cut => ("切り取り", "Cut"),
            Native::Copy => ("コピー", "Copy"),
            Native::Paste => ("貼り付け", "Paste"),
            Native::SelectAll => ("すべてを選択", "Select All"),
            Native::Fullscreen => ("フルスクリーンにする", "Enter Full Screen"),
            Native::Minimize => ("しまう", "Minimize"),
            Native::Zoom => ("拡大/縮小", "Zoom"),
            Native::BringAllToFront => ("すべてを手前に移動", "Bring All to Front"),
        };
        match lang {
            Lang::Ja => ja,
            Lang::En => en,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Title {
    /// macOS's menu named after the app.
    App,
    File,
    Edit,
    View,
    Window,
    Help,
}

impl Title {
    pub fn text(self, lang: Lang) -> &'static str {
        let (ja, en) = match self {
            Title::App => ("jamuru", "jamuru"),
            Title::File => ("ファイル", "File"),
            Title::Edit => ("編集", "Edit"),
            Title::View => ("表示", "View"),
            Title::Window => ("ウィンドウ", "Window"),
            Title::Help => ("ヘルプ", "Help"),
        };
        match lang {
            Lang::Ja => ja,
            Lang::En => en,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    Item(Item),
    Native(Native),
    Separator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: Title,
    pub nodes: Vec<Node>,
}

/// Which of the two layouts: macOS has the app-name menu, a Window menu and
/// the Help search; Windows and Linux have the plain File / Edit / Help.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Other
        }
    }
}

/// The menu bar, left to right.
pub fn layout(os: Os) -> Vec<Section> {
    use Node::{Item as I, Native as N, Separator as S};
    let room = [
        I(Item::CreateRoom),
        I(Item::JoinRoom),
        I(Item::TestRoom),
        S,
        I(Item::CopyInviteCode),
        I(Item::LeaveRoom),
        I(Item::AskForHelp),
    ];
    let help = [
        I(Item::Guide),
        I(Item::Troubleshooting),
        S,
        I(Item::Diagnostics),
        I(Item::OpenLogFolder),
        I(Item::ReportProblem),
        S,
        I(Item::Terms),
        I(Item::Privacy),
        I(Item::Licenses),
        I(Item::Announcements),
    ];
    match os {
        Os::Mac => vec![
            Section {
                title: Title::App,
                nodes: vec![
                    N(Native::About),
                    S,
                    I(Item::Settings),
                    S,
                    N(Native::Services),
                    S,
                    N(Native::Hide),
                    N(Native::HideOthers),
                    N(Native::ShowAll),
                    S,
                    N(Native::Quit),
                ],
            },
            Section {
                title: Title::File,
                nodes: room
                    .into_iter()
                    .chain([S, N(Native::CloseWindow)])
                    .collect(),
            },
            Section {
                title: Title::Edit,
                nodes: vec![
                    N(Native::Undo),
                    N(Native::Redo),
                    S,
                    N(Native::Cut),
                    N(Native::Copy),
                    N(Native::Paste),
                    N(Native::SelectAll),
                ],
            },
            Section {
                title: Title::View,
                nodes: vec![N(Native::Fullscreen)],
            },
            Section {
                title: Title::Window,
                nodes: vec![
                    N(Native::Minimize),
                    N(Native::Zoom),
                    S,
                    N(Native::BringAllToFront),
                ],
            },
            Section {
                title: Title::Help,
                nodes: help.to_vec(),
            },
        ],
        Os::Other => vec![
            Section {
                title: Title::File,
                nodes: room
                    .into_iter()
                    .chain([S, I(Item::Settings), S, I(Item::Quit)])
                    .collect(),
            },
            Section {
                title: Title::Edit,
                nodes: vec![
                    N(Native::Cut),
                    N(Native::Copy),
                    N(Native::Paste),
                    N(Native::SelectAll),
                ],
            },
            Section {
                title: Title::Help,
                nodes: help.into_iter().chain([S, N(Native::About)]).collect(),
            },
        ],
    }
}

/// The menu as the OS has it, with the entries the app made kept so their
/// state can change without building the menu again.
pub struct MenuState {
    inner: Mutex<Built>,
}

struct Built {
    lang: Lang,
    window: WindowKind,
    items: Vec<Made>,
}

/// An entry the app made, and whether it is enabled now.
struct Made {
    item: Item,
    entry: MenuItem<Wry>,
    enabled: bool,
}

fn build(app: &AppHandle, lang: Lang, context: &Context) -> tauri::Result<(Menu<Wry>, Vec<Made>)> {
    let mut made = Vec::new();
    let mut submenus = Vec::new();
    for section in layout(Os::current()) {
        let mut entries: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
        for node in &section.nodes {
            entries.push(match *node {
                Node::Separator => Box::new(PredefinedMenuItem::separator(app)?),
                Node::Item(item) => {
                    let enabled = item.enabled(context);
                    let entry =
                        MenuItem::with_id(app, item.id(), item.text(lang), enabled, None::<&str>)?;
                    made.push(Made {
                        item,
                        entry: entry.clone(),
                        enabled,
                    });
                    Box::new(entry)
                }
                Node::Native(native) => native_entry(app, native, lang)?,
            });
        }
        let refs: Vec<&dyn IsMenuItem<Wry>> = entries.iter().map(|e| e.as_ref()).collect();
        let text = section.title.text(lang);
        submenus.push(match (Os::current(), section.title) {
            (Os::Mac, Title::Window) => {
                Submenu::with_id_and_items(app, WINDOW_SUBMENU_ID, text, true, &refs)?
            }
            (Os::Mac, Title::Help) => {
                Submenu::with_id_and_items(app, HELP_SUBMENU_ID, text, true, &refs)?
            }
            _ => Submenu::with_items(app, text, true, &refs)?,
        });
    }
    let refs: Vec<&dyn IsMenuItem<Wry>> = submenus.iter().map(|s| s as _).collect();
    Ok((Menu::with_items(app, &refs)?, made))
}

fn native_entry(
    app: &AppHandle,
    native: Native,
    lang: Lang,
) -> tauri::Result<Box<dyn IsMenuItem<Wry>>> {
    let text = Some(native.text(lang));
    Ok(match native {
        Native::About => Box::new(PredefinedMenuItem::about(
            app,
            text,
            Some(AboutMetadata {
                name: Some("jamuru".to_string()),
                version: Some(app.package_info().version.to_string()),
                copyright: app.config().bundle.copyright.clone(),
                ..Default::default()
            }),
        )?),
        Native::Services => Box::new(PredefinedMenuItem::services(app, text)?),
        Native::Hide => Box::new(PredefinedMenuItem::hide(app, text)?),
        Native::HideOthers => Box::new(PredefinedMenuItem::hide_others(app, text)?),
        Native::ShowAll => Box::new(PredefinedMenuItem::show_all(app, text)?),
        Native::Quit => Box::new(PredefinedMenuItem::quit(app, text)?),
        Native::CloseWindow => Box::new(PredefinedMenuItem::close_window(app, text)?),
        Native::Undo => Box::new(PredefinedMenuItem::undo(app, text)?),
        Native::Redo => Box::new(PredefinedMenuItem::redo(app, text)?),
        Native::Cut => Box::new(PredefinedMenuItem::cut(app, text)?),
        Native::Copy => Box::new(PredefinedMenuItem::copy(app, text)?),
        Native::Paste => Box::new(PredefinedMenuItem::paste(app, text)?),
        Native::SelectAll => Box::new(PredefinedMenuItem::select_all(app, text)?),
        Native::Fullscreen => Box::new(PredefinedMenuItem::fullscreen(app, text)?),
        Native::Minimize => Box::new(PredefinedMenuItem::minimize(app, text)?),
        Native::Zoom => Box::new(PredefinedMenuItem::maximize(app, text)?),
        Native::BringAllToFront => Box::new(PredefinedMenuItem::bring_all_to_front(app, text)?),
    })
}

fn current_lang(app: &AppHandle) -> Lang {
    let chosen = app
        .try_state::<ConfigState>()
        .and_then(|config| config.get().ok())
        .and_then(|config| config.language);
    Lang::resolve(chosen.as_deref(), sys_locale::get_locale().as_deref())
}

/// Puts the menu on the app and has it follow the app from here on: the
/// session, the language, the terms and the window in front each change
/// which entries can be used. After the state the menu reads is managed.
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let lang = current_lang(app);
    let window = WindowKind::Main;
    let (menu, items) = build(app, lang, &Context::from_app(app, window))?;
    app.set_menu(menu)?;
    app.manage(MenuState {
        inner: Mutex::new(Built {
            lang,
            window,
            items,
        }),
    });

    for event in [crate::session::CHANGED_EVENT, "i18n:language-changed"] {
        let handle = app.clone();
        app.listen(event, move |_| refresh(&handle, None));
    }
    // Until the terms are agreed to, most entries are disabled (REQ-TRM-003).
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::terms::wait_accepted(&handle).await;
        refresh(&handle, None);
    });
    Ok(())
}

/// Brings the menu up to date with the app: builds it again when the language
/// changed, and otherwise only enables or disables its entries. `window` is
/// the window that came to the front, if one did.
///
/// The lock is never held while the OS is asked for anything: the OS's menu
/// is changed on the main thread, which may itself be waiting for this lock.
pub fn refresh(app: &AppHandle, window: Option<WindowKind>) {
    let Some(state) = app.try_state::<MenuState>() else {
        return;
    };
    let lang = current_lang(app);
    let (context, rebuild, stale) = {
        let Ok(mut built) = state.inner.lock() else {
            return;
        };
        if let Some(window) = window {
            built.window = window;
        }
        let context = Context::from_app(app, built.window);
        let stale: Vec<(MenuItem<Wry>, bool)> = built
            .items
            .iter_mut()
            .filter_map(|made| {
                let enabled = made.item.enabled(&context);
                let changed = enabled != made.enabled;
                made.enabled = enabled;
                changed.then(|| (made.entry.clone(), enabled))
            })
            .collect();
        (context, lang != built.lang, stale)
    };
    if rebuild {
        match build(app, lang, &context).and_then(|(menu, items)| {
            app.set_menu(menu)?;
            Ok(items)
        }) {
            Ok(items) => {
                if let Ok(mut built) = state.inner.lock() {
                    built.lang = lang;
                    built.items = items;
                }
            }
            Err(e) => tracing::warn!("Could not rebuild the menu: {}", e),
        }
        return;
    }
    for (entry, enabled) in stale {
        if let Err(e) = entry.set_enabled(enabled) {
            tracing::warn!("Could not update a menu entry: {}", e);
        }
    }
}

/// A window came to the front: the menu is read from it now.
pub fn window_focused(app: &AppHandle, label: &str) {
    refresh(app, Some(WindowKind::of(label)));
}

/// An entry of the menu was chosen.
pub fn handle_event(app: &AppHandle, id: &str) {
    let Some(item) = Item::from_id(id) else {
        // The OS's own entries carry themselves out.
        return;
    };
    match item.route() {
        Route::Screen(action) => {
            // The screen that does it is the main window's; it may be behind.
            if let Some(main) = app.get_webview_window(windows::labels::MAIN) {
                let _ = main.unminimize();
                let _ = main.set_focus();
            }
            if let Err(e) = app.emit_to(windows::labels::MAIN, ACTION_EVENT, action) {
                tracing::warn!("Could not hand {} to the screen: {}", id, e);
            }
        }
        Route::Settings(tab) => {
            // A window built on the main thread, where menu events arrive,
            // deadlocks WebView2.
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = windows::create_settings_window_at(&app, tab) {
                    tracing::warn!("Could not open the settings window: {}", e);
                }
            });
        }
        Route::Browser(url) => {
            if let Err(e) = crate::terms::open_url(url) {
                tracing::warn!("Could not open {}: {}", url, e);
            }
        }
        Route::LogFolder => {
            if let Err(e) = crate::logging::log_open_dir(app.clone()) {
                tracing::warn!("Could not open the log folder: {}", e);
            }
        }
        Route::Exit => app.exit(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether Windows and Linux have the entry. The ones they lack are macOS's.
    fn on_other_os(native: Native) -> bool {
        matches!(
            native,
            Native::About | Native::Cut | Native::Copy | Native::Paste | Native::SelectAll
        )
    }

    fn context() -> Context {
        Context {
            window: WindowKind::Main,
            terms_accepted: true,
            can_enter_room: true,
            test_room: true,
            in_room: false,
        }
    }

    fn items_of(os: Os) -> Vec<Item> {
        layout(os)
            .into_iter()
            .flat_map(|section| section.nodes)
            .filter_map(|node| match node {
                Node::Item(item) => Some(item),
                _ => None,
            })
            .collect()
    }

    /// Verifies: REQ-MNU-001
    #[test]
    fn on_windows_and_linux_every_entry_is_in_the_menu_once() {
        let mut items = items_of(Os::Other);
        items.sort_by_key(|item| item.id());
        let mut all = Item::ALL.to_vec();
        all.sort_by_key(|item| item.id());
        assert_eq!(items, all);
    }

    /// Verifies: REQ-MNU-001
    #[test]
    fn on_macos_every_entry_but_quit_is_in_the_menu_once() {
        let mut items = items_of(Os::Mac);
        items.sort_by_key(|item| item.id());
        let mut expected: Vec<Item> = Item::ALL.into_iter().filter(|i| *i != Item::Quit).collect();
        expected.sort_by_key(|item| item.id());
        assert_eq!(items, expected);
        // macOS quits from the app-name menu, with the OS's own entry.
        assert!(layout(Os::Mac)
            .iter()
            .any(|s| s.nodes.contains(&Node::Native(Native::Quit))));
    }

    /// Verifies: REQ-MNU-001
    #[test]
    fn on_windows_and_linux_the_menu_holds_only_what_those_systems_provide() {
        for section in layout(Os::Other) {
            for node in section.nodes {
                if let Node::Native(native) = node {
                    assert!(on_other_os(native), "{:?} is macOS's", native);
                }
            }
        }
    }

    /// Verifies: REQ-MNU-002
    #[test]
    fn on_macos_the_app_name_menu_comes_first_and_help_comes_last() {
        let sections = layout(Os::Mac);
        assert_eq!(sections.first().map(|s| s.title), Some(Title::App));
        assert_eq!(sections.last().map(|s| s.title), Some(Title::Help));
        assert!(sections.iter().any(|s| s.title == Title::Window
            && s.nodes.contains(&Node::Native(Native::BringAllToFront))));
    }

    /// Verifies: REQ-MNU-002
    #[test]
    fn on_windows_and_linux_settings_is_in_file_and_about_is_in_help() {
        let sections = layout(Os::Other);
        let file = sections.iter().find(|s| s.title == Title::File).unwrap();
        let help = sections.iter().find(|s| s.title == Title::Help).unwrap();
        assert!(file.nodes.contains(&Node::Item(Item::Settings)));
        assert!(help.nodes.contains(&Node::Native(Native::About)));
    }

    /// Verifies: REQ-MNU-003
    #[test]
    fn the_help_menu_reaches_the_guide_the_support_tools_and_the_notices() {
        for os in [Os::Mac, Os::Other] {
            let help = layout(os)
                .into_iter()
                .find(|s| s.title == Title::Help)
                .unwrap();
            for item in [
                Item::Guide,
                Item::Troubleshooting,
                Item::Diagnostics,
                Item::OpenLogFolder,
                Item::ReportProblem,
                Item::Terms,
                Item::Privacy,
                Item::Licenses,
                Item::Announcements,
            ] {
                assert!(
                    help.nodes.contains(&Node::Item(item)),
                    "{:?} on {:?}",
                    item,
                    os
                );
            }
        }
    }

    /// Verifies: REQ-MNU-004
    #[test]
    fn every_entry_has_an_id_that_finds_it_and_text_in_both_languages() {
        let mut ids = std::collections::HashSet::new();
        for item in Item::ALL {
            assert!(ids.insert(item.id()), "{} is used twice", item.id());
            assert_eq!(Item::from_id(item.id()), Some(item));
            assert!(!item.text(Lang::Ja).is_empty() && !item.text(Lang::En).is_empty());
            assert_ne!(item.text(Lang::Ja), item.text(Lang::En));
        }
        assert_eq!(Item::from_id("menu.nothing"), None);
    }

    /// Verifies: REQ-MNU-004
    #[test]
    fn every_native_entry_has_text_in_both_languages() {
        for os in [Os::Mac, Os::Other] {
            for section in layout(os) {
                for node in section.nodes {
                    if let Node::Native(native) = node {
                        assert!(!native.text(Lang::Ja).is_empty());
                        assert!(!native.text(Lang::En).is_empty());
                    }
                }
            }
        }
    }

    /// Verifies: REQ-MNU-004
    #[test]
    fn the_chosen_language_decides_the_menu_language_before_the_system_does() {
        assert_eq!(Lang::resolve(Some("ja"), Some("en-US")), Lang::Ja);
        assert_eq!(Lang::resolve(Some("en"), Some("ja-JP")), Lang::En);
    }

    /// Verifies: REQ-MNU-004
    #[test]
    fn with_no_chosen_language_the_system_language_decides_and_english_is_the_last_resort() {
        assert_eq!(Lang::resolve(None, Some("ja-JP")), Lang::Ja);
        assert_eq!(Lang::resolve(None, Some("fr-FR")), Lang::En);
        assert_eq!(Lang::resolve(None, None), Lang::En);
    }

    /// Verifies: REQ-MNU-005
    #[test]
    fn before_a_room_is_entered_a_room_can_be_made_or_joined_and_not_left() {
        let context = context();
        for item in [Item::CreateRoom, Item::JoinRoom, Item::TestRoom] {
            assert!(item.enabled(&context), "{:?}", item);
        }
        for item in [Item::CopyInviteCode, Item::LeaveRoom, Item::AskForHelp] {
            assert!(!item.enabled(&context), "{:?}", item);
        }
    }

    /// Verifies: REQ-MNU-005
    #[test]
    fn in_a_room_the_room_can_be_left_and_not_made() {
        let context = Context {
            in_room: true,
            can_enter_room: false,
            ..context()
        };
        for item in [Item::CopyInviteCode, Item::LeaveRoom, Item::AskForHelp] {
            assert!(item.enabled(&context), "{:?}", item);
        }
        for item in [Item::CreateRoom, Item::JoinRoom, Item::TestRoom] {
            assert!(!item.enabled(&context), "{:?}", item);
        }
    }

    /// Verifies: REQ-MNU-005
    #[test]
    fn without_a_test_room_from_the_server_the_test_room_is_not_offered() {
        let context = Context {
            test_room: false,
            ..context()
        };
        assert!(!Item::TestRoom.enabled(&context));
        assert!(Item::CreateRoom.enabled(&context));
    }

    /// Verifies: REQ-MNU-006
    #[test]
    fn before_the_terms_are_agreed_to_only_what_stays_on_the_machine_or_in_the_browser_works() {
        let context = Context {
            terms_accepted: false,
            ..context()
        };
        for item in Item::ALL {
            let expected = matches!(
                item,
                Item::Quit
                    | Item::Guide
                    | Item::Troubleshooting
                    | Item::OpenLogFolder
                    | Item::Privacy
                    | Item::Announcements
            );
            assert_eq!(item.enabled(&context), expected, "{:?}", item);
        }
    }

    /// Verifies: REQ-MNU-006
    #[test]
    fn from_a_helpers_window_only_the_browser_and_the_log_folder_entries_work() {
        let context = Context {
            window: WindowKind::Help,
            in_room: true,
            can_enter_room: false,
            ..context()
        };
        for item in Item::ALL {
            let expected = matches!(
                item,
                Item::Quit
                    | Item::Guide
                    | Item::Troubleshooting
                    | Item::OpenLogFolder
                    | Item::Privacy
                    | Item::Announcements
            );
            assert_eq!(item.enabled(&context), expected, "{:?}", item);
        }
    }

    /// Verifies: REQ-MNU-006
    #[test]
    fn from_the_settings_window_the_settings_entries_work_and_the_room_ones_do_not() {
        let context = Context {
            window: WindowKind::Settings,
            in_room: true,
            ..context()
        };
        for item in [
            Item::Settings,
            Item::Diagnostics,
            Item::ReportProblem,
            Item::Terms,
            Item::Licenses,
        ] {
            assert!(item.enabled(&context), "{:?}", item);
        }
        for item in [
            Item::CreateRoom,
            Item::CopyInviteCode,
            Item::LeaveRoom,
            Item::AskForHelp,
        ] {
            assert!(!item.enabled(&context), "{:?}", item);
        }
    }

    /// Verifies: REQ-MNU-007
    #[test]
    fn a_window_is_told_apart_by_its_label() {
        assert_eq!(WindowKind::of("main"), WindowKind::Main);
        assert_eq!(WindowKind::of("settings"), WindowKind::Settings);
        assert_eq!(WindowKind::of("help-3"), WindowKind::Help);
        assert_eq!(WindowKind::of("help-log"), WindowKind::Help);
    }

    /// Verifies: REQ-MNU-007
    #[test]
    fn entries_that_open_settings_name_a_tab_the_settings_window_has() {
        for item in Item::ALL {
            if let Route::Settings(Some(tab)) = item.route() {
                assert!(windows::SETTINGS_TABS.contains(&tab), "{:?}", item);
            }
        }
        assert_eq!(
            Item::ReportProblem.route(),
            Route::Settings(Some("diagnostics"))
        );
        assert_eq!(Item::Terms.route(), Route::Settings(Some("about")));
    }

    /// Verifies: REQ-MNU-007
    #[test]
    fn entries_that_work_on_the_screen_each_name_a_different_action() {
        let mut actions = std::collections::HashSet::new();
        for item in Item::ALL {
            if let Route::Screen(action) = item.route() {
                assert!(actions.insert(action), "{} is used twice", action);
            }
        }
        assert_eq!(actions.len(), 6);
    }

    /// Verifies: REQ-MNU-007
    #[test]
    fn entries_that_open_the_browser_open_https_pages() {
        for item in Item::ALL {
            if let Route::Browser(url) = item.route() {
                assert_eq!(url.split_once("://").map(|(scheme, _)| scheme), Some("https"), "{:?}", item);
            }
        }
    }
}
