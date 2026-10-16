//! The differences between the menu bar on macOS and on Windows / Linux, and why each exists.
//!
//! The menu is meant to be the same everywhere: the same entries, in the same
//! menu, under the same name. Where the OS's own habit calls for something
//! else, the difference is written down here with its reason. A difference
//! that is not in this table fails the tests, and so does a row that no longer
//! describes the menu.

use super::{layout, Item, Lang, Native, Node, Os, Title};

/// An entry of the menu bar as a person sees it, whichever OS it is on. The
/// OS's own "Quit" on macOS and the one made here elsewhere are the same entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Menu(Title),
    Item(Item),
    Native(Native),
}

impl Key {
    fn of(node: Node) -> Option<Key> {
        match node {
            Node::Separator => None,
            Node::Item(item) => Some(Key::Item(item)),
            Node::Native(Native::Quit) => Some(Key::Item(Item::Quit)),
            Node::Native(native) => Some(Key::Native(native)),
        }
    }
}

/// One row of the ledger.
pub struct Difference {
    pub key: Key,
    /// The menu the entry is in on macOS (the entry's own title for a menu).
    pub mac: Option<Title>,
    /// The same on Windows / Linux.
    pub other: Option<Title>,
    /// Whether the entry reads the same on both, where it is on both.
    pub same_name: bool,
    pub reason: &'static str,
}

const MAC_ONLY_ITEM: &str = "macOS 専用の項目。Windows / Linux には同じ働きの標準がない";

pub const DIFFERENCES: &[Difference] = &[
    Difference {
        key: Key::Menu(Title::App),
        mac: Some(Title::App),
        other: None,
        same_name: true,
        reason: "macOS はアプリ名のメニューに、アプリについて・設定・隠す・終了を置く決まり。Windows / Linux にこのメニューは無く、項目はファイルとヘルプに置く",
    },
    Difference {
        key: Key::Menu(Title::View),
        mac: Some(Title::View),
        other: None,
        same_name: true,
        reason: "macOS はフルスクリーンの切り替えを表示メニューに置く。Windows / Linux はアプリが出す表示の項目が無い（フルスクリーンは OS 標準の項目として組み込めない）",
    },
    Difference {
        key: Key::Menu(Title::Window),
        mac: Some(Title::Window),
        other: None,
        same_name: true,
        reason: "macOS のウィンドウメニューは、しまう・拡大/縮小・開いているウィンドウの一覧を OS が扱う。Windows / Linux にこのメニューの習慣は無く、ウィンドウの操作は枠のボタンで行う",
    },
    Difference {
        key: Key::Item(Item::Settings),
        mac: Some(Title::App),
        other: Some(Title::File),
        same_name: true,
        reason: "macOS はアプリ名のメニューの「設定…」に置く決まり。Windows / Linux には設定専用のメニューが無く（ツールやオプションに当たるメニューを持たない）、ファイルの下に置く",
    },
    Difference {
        key: Key::Item(Item::Quit),
        mac: Some(Title::App),
        other: Some(Title::File),
        same_name: false,
        reason: "macOS はアプリ名のメニューにある OS 標準の「jamuru を終了」。Windows はファイルの末尾に「終了」を置く習慣。Linux には OS 標準の終了の項目が無いので、どちらもアプリの項目にした",
    },
    Difference {
        key: Key::Native(Native::About),
        mac: Some(Title::App),
        other: Some(Title::Help),
        same_name: true,
        reason: "macOS はアプリ名のメニューの先頭に「jamuru について」を置く決まり。Windows / Linux はヘルプの末尾に置く習慣",
    },
    Difference {
        key: Key::Item(Item::CheckForUpdates),
        mac: Some(Title::App),
        other: Some(Title::Help),
        same_name: true,
        reason: "macOS はアプリ名のメニューの「jamuru について」の直下に更新の確認を置く決まり。Windows / Linux はヘルプの末尾、「jamuru について」の直前に置く習慣",
    },
    Difference {
        key: Key::Native(Native::Services),
        mac: Some(Title::App),
        other: None,
        same_name: true,
        reason: MAC_ONLY_ITEM,
    },
    Difference {
        key: Key::Native(Native::Hide),
        mac: Some(Title::App),
        other: None,
        same_name: true,
        reason: MAC_ONLY_ITEM,
    },
    Difference {
        key: Key::Native(Native::HideOthers),
        mac: Some(Title::App),
        other: None,
        same_name: true,
        reason: MAC_ONLY_ITEM,
    },
    Difference {
        key: Key::Native(Native::ShowAll),
        mac: Some(Title::App),
        other: None,
        same_name: true,
        reason: MAC_ONLY_ITEM,
    },
    Difference {
        key: Key::Native(Native::CloseWindow),
        mac: Some(Title::File),
        other: None,
        same_name: true,
        reason: "macOS はファイルに「ウィンドウを閉じる」を置く。Windows は枠の閉じるボタンと Alt+F4 で閉じ、Linux には OS 標準の項目が無い",
    },
    Difference {
        key: Key::Native(Native::Undo),
        mac: Some(Title::Edit),
        other: None,
        same_name: true,
        reason: "macOS の標準の編集メニューにある。Windows / Linux では OS 標準の項目として組み込めない（入力欄の取り消しはキーボードで効く）",
    },
    Difference {
        key: Key::Native(Native::Redo),
        mac: Some(Title::Edit),
        other: None,
        same_name: true,
        reason: "macOS の標準の編集メニューにある。Windows / Linux では OS 標準の項目として組み込めない（入力欄のやり直しはキーボードで効く）",
    },
    Difference {
        key: Key::Native(Native::Fullscreen),
        mac: Some(Title::View),
        other: None,
        same_name: true,
        reason: "macOS の標準の表示メニューにある。Windows / Linux では OS 標準の項目として組み込めない",
    },
    Difference {
        key: Key::Native(Native::Minimize),
        mac: Some(Title::Window),
        other: None,
        same_name: true,
        reason: "macOS はウィンドウメニューに置く。Windows / Linux は枠の最小化ボタンで行う",
    },
    Difference {
        key: Key::Native(Native::Zoom),
        mac: Some(Title::Window),
        other: None,
        same_name: true,
        reason: "macOS 専用の項目（ウィンドウを内容に合わせて拡大・縮小）。Windows / Linux は枠の最大化ボタンで行う",
    },
    Difference {
        key: Key::Native(Native::BringAllToFront),
        mac: Some(Title::Window),
        other: None,
        same_name: true,
        reason: MAC_ONLY_ITEM,
    },
];

/// The menu `key` is in on `os` and what it reads, in `lang`.
fn find(os: Os, key: Key, lang: Lang) -> Option<(Title, String)> {
    for section in layout(os) {
        if key == Key::Menu(section.title) {
            return Some((section.title, section.title.text(lang).to_string()));
        }
        for node in &section.nodes {
            if Key::of(*node) != Some(key) {
                continue;
            }
            let name = match node {
                Node::Item(item) => item.text(lang),
                Node::Native(native) => native.text(lang),
                Node::Separator => continue,
            };
            return Some((section.title, name.to_string()));
        }
    }
    None
}

/// Every entry that is in the menu on either OS: the macOS menu first, then
/// what only Windows / Linux has.
fn all_keys() -> Vec<Key> {
    let mut keys = Vec::new();
    for os in [Os::Mac, Os::Other] {
        for section in layout(os) {
            for key in std::iter::once(Key::Menu(section.title))
                .chain(section.nodes.iter().filter_map(|node| Key::of(*node)))
            {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
    }
    keys
}

fn name_of(key: Key) -> &'static str {
    match key {
        Key::Menu(title) => title.text(Lang::Ja),
        Key::Item(item) => item.text(Lang::Ja),
        Key::Native(native) => native.text(Lang::Ja),
    }
}

/// The menu the entry is in on both, and whether it reads the same in both
/// languages. `None` for the menu where the entry is missing.
fn actual(key: Key) -> (Option<Title>, Option<Title>, bool) {
    let mac = find(Os::Mac, key, Lang::Ja);
    let other = find(Os::Other, key, Lang::Ja);
    let same_name = [Lang::Ja, Lang::En].into_iter().all(|lang| {
        match (find(Os::Mac, key, lang), find(Os::Other, key, lang)) {
            (Some((_, a)), Some((_, b))) => a == b,
            _ => true,
        }
    });
    (mac.map(|m| m.0), other.map(|o| o.0), same_name)
}

fn differs(key: Key) -> bool {
    let (mac, other, same_name) = actual(key);
    mac != other || !same_name
}

/// The whole ledger as the Wiki page's body, every entry with where it is on
/// each OS and, where they differ, why.
fn ledger_markdown() -> String {
    let place = |os: Os, key: Key| match find(os, key, Lang::Ja) {
        None => "なし".to_string(),
        Some((title, name)) if key == Key::Menu(title) => format!("**{name}**"),
        Some((title, name)) => format!("{} › {}", title.text(Lang::Ja), name),
    };
    let mut out = String::new();
    for (heading, menus) in [("メニュー（画面上端に並ぶ見出し）", true), ("項目", false)]
    {
        out.push_str(&format!(
            "\n## {heading}\n\n| 項目 | macOS | Windows / Linux | 差分 | 理由 |\n|---|---|---|---|---|\n"
        ));
        for key in all_keys()
            .into_iter()
            .filter(|key| matches!(key, Key::Menu(_)) == menus)
        {
            let (verdict, reason) = match DIFFERENCES.iter().find(|row| row.key == key) {
                Some(row) => ("あり", row.reason),
                None => ("なし", "-"),
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                name_of(key),
                place(Os::Mac, key),
                place(Os::Other, key),
                verdict,
                reason
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: REQ-MNU-009
    #[test]
    fn a_difference_between_the_systems_is_in_the_ledger_with_a_reason() {
        let in_the_menu: Vec<Key> = all_keys().into_iter().filter(|k| differs(*k)).collect();
        let in_the_ledger: Vec<Key> = DIFFERENCES.iter().map(|row| row.key).collect();

        for key in &in_the_menu {
            assert!(
                in_the_ledger.contains(key),
                "{:?} differs between macOS and Windows / Linux and the ledger has no row for it",
                key
            );
        }
        for row in DIFFERENCES {
            assert!(
                in_the_menu.contains(&row.key),
                "the ledger has a row for {:?}, which no longer differs",
                row.key
            );
            assert!(
                row.reason.chars().count() >= 10,
                "the row for {:?} gives no reason",
                row.key
            );
        }
    }

    /// Verifies: REQ-MNU-009
    #[test]
    fn a_row_of_the_ledger_says_where_the_entry_is_and_whether_the_name_differs() {
        for row in DIFFERENCES {
            let (mac, other, same_name) = actual(row.key);
            assert_eq!(row.mac, mac, "{:?} on macOS", row.key);
            assert_eq!(row.other, other, "{:?} on Windows / Linux", row.key);
            assert_eq!(row.same_name, same_name, "{:?} name", row.key);
        }
    }

    /// Verifies: REQ-MNU-009
    #[test]
    fn the_ledger_has_one_row_for_an_entry() {
        for (i, row) in DIFFERENCES.iter().enumerate() {
            assert!(
                DIFFERENCES[i + 1..]
                    .iter()
                    .all(|other| other.key != row.key),
                "{:?} has two rows",
                row.key
            );
        }
    }

    /// Verifies: REQ-MNU-009
    #[test]
    fn entries_on_both_systems_come_in_the_same_order() {
        let order = |os: Os, title: Title| -> Vec<Key> {
            layout(os)
                .into_iter()
                .filter(|section| section.title == title)
                .flat_map(|section| section.nodes)
                .filter_map(Key::of)
                .collect()
        };
        for title in [Title::File, Title::Edit, Title::Help] {
            let mac = order(Os::Mac, title);
            let other = order(Os::Other, title);
            let in_both = |keys: &[Key], against: &[Key]| -> Vec<Key> {
                keys.iter()
                    .copied()
                    .filter(|k| against.contains(k))
                    .collect()
            };
            assert_eq!(
                in_both(&mac, &other),
                in_both(&other, &mac),
                "{:?} lists the shared entries in a different order",
                title
            );
        }
    }

    /// Verifies: REQ-MNU-009
    #[test]
    fn every_entry_is_in_the_menu_bar_of_at_least_one_system() {
        for item in Item::ALL {
            assert!(all_keys().contains(&Key::Item(item)), "{:?}", item);
        }
    }

    /// Writes the ledger where `JAMJAM_MENU_LEDGER_OUT` points, for the Wiki page.
    #[test]
    #[ignore = "writes the whole ledger where JAMJAM_MENU_LEDGER_OUT points"]
    fn write_the_whole_menu_ledger() {
        let out = std::env::var_os("JAMJAM_MENU_LEDGER_OUT")
            .expect("JAMJAM_MENU_LEDGER_OUT names the file to write");
        std::fs::write(out, ledger_markdown()).expect("the ledger can be written");
    }
}
