//! Grep tests for the documents about the customizable sidebar (plan
//! sidebar-customization, Task 13): `docs/sidebar.md` and the sidebar section
//! of `docs/accessibility.md` must say what the acceptance criteria say, and
//! must not promise what does not exist (a sync of the layout, or a file that
//! is removed at logout).

use twicecat_app::sidebar_menu::Entry;
use twicecat_core::api::SidebarLabel;
use twicecat_core::api::sidebar::limits;

fn doc(name: &str) -> String {
    let path = format!("{}/../../docs/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path} is readable: {e}"))
}

/// The text with line breaks and runs of blanks folded to single spaces and
/// letters lowercased, so a phrase wrapped over two lines is still found.
fn flat(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The sidebar section of `accessibility.md`.
fn accessibility_sidebar_section() -> String {
    let text = doc("accessibility.md");
    let start = text
        .find("## The sidebar")
        .expect("accessibility.md has a sidebar section");
    let rest = &text[start + 3..];
    let end = rest.find("\n## ").map_or(rest.len(), |at| at + 1);
    rest[..end].to_owned()
}

const ENTRIES: [Entry; 12] = [
    Entry::MoveUp,
    Entry::MoveDown,
    Entry::RemoveFromFolder,
    Entry::Rename,
    Entry::Ungroup,
    Entry::Star,
    Entry::Unstar,
    Entry::BackToDefault,
    Entry::Collapse,
    Entry::Expand,
    Entry::Delete,
    Entry::NewSection,
];

#[test]
fn accessibility_names_the_options_menus_and_says_they_need_a_pointer() {
    let section = flat(&accessibility_sidebar_section());
    for wanted in [
        "options menus",
        "right-click",
        "\u{22EF}",
        "alternative to dragging",
        "need a pointer",
        "iced 0.14",
        "buttons take no keyboard focus",
        "keyboard-only users",
        "keep the arrangement but cannot change it",
        "no keyboard chord",
        "pointer-only",
        "auto-scroll",
    ] {
        assert!(
            section.contains(wanted),
            "the sidebar section of accessibility.md does not say {wanted:?}"
        );
    }
    // Every move, folder move, section move and star is an entry of the menu.
    for entry in ENTRIES.iter().filter(|e| **e != Entry::NewSection) {
        let label = entry.label().trim_end_matches('\u{2026}').to_lowercase();
        assert!(section.contains(&label), "{label:?} is not named");
    }
    for wanted in ["move to folder", "move to section"] {
        assert!(section.contains(wanted), "{wanted:?} is not named");
    }
}

#[test]
fn the_keyboard_map_of_accessibility_gets_no_sidebar_row() {
    // The table is the one `SHORTCUTS` drives (a test in `keys.rs` compares
    // them). The sidebar adds no chord, so no table row mentions it.
    let text = doc("accessibility.md");
    let table: Vec<&str> = text
        .lines()
        .skip_while(|l| !l.starts_with("## Keyboard map"))
        .skip(1)
        .take_while(|l| !l.starts_with("Rules the code follows"))
        .filter(|l| l.starts_with('|'))
        .collect();
    assert!(table.len() > 10, "the table was found");
    for row in table {
        let row = row.to_lowercase();
        for word in ["sidebar", "folder", "section", "starred"] {
            assert!(!row.contains(word), "a table row mentions {word}: {row}");
        }
    }
}

#[test]
fn sidebar_md_says_where_the_layout_is_and_for_whom() {
    let text = doc("sidebar.md");
    let flat = flat(&text);
    for wanted in [
        "sidebar-<digest>.json",
        "settings.json",
        "this device only",
        "one file per account",
        // Kept at logout and read again at the next login.
        "the file is kept when you log out",
        "read again the next time the same account logs in",
        "another account on the same device never reads or changes your file",
        // How to remove it.
        "reset sidebar layout",
        "settings, appearance",
        "delete the file",
        // The two protected states.
        "made by a newer twicecat",
        "belongs to another account",
        "left exactly as they are",
        "is off",
        // The ways to arrange.
        "options menus",
        "right-click",
        "\u{22EF}",
        "drag",
    ] {
        assert!(flat.contains(wanted), "sidebar.md does not say {wanted:?}");
    }
}

#[test]
fn sidebar_md_claims_no_sync_and_no_removal_at_logout() {
    let flat = flat(&doc("sidebar.md"));
    // No claim of a sync, in any form: the layout never leaves the device.
    for word in [
        "sync",
        "synchron",
        "cloud",
        "account data",
        "account-data",
        "encrypt",
        "end-to-end",
    ] {
        assert!(!flat.contains(word), "sidebar.md mentions {word:?}");
    }
    for phrase in [
        "removed at logout",
        "removed at log out",
        "removed when you log out",
        "removed on logout",
        "removed on log out",
        "removed when you sign out",
        "removed at sign out",
        "deleted at logout",
        "deleted at log out",
        "deleted when you log out",
        "deleted when you sign out",
        "deleted on logout",
        "erased at logout",
        "erased when you log out",
        "cleared at logout",
        "cleared when you log out",
        "wiped at logout",
        "wiped when you log out",
        "forgotten at logout",
        "is lost when you log out",
        "removes the file when you log out",
        "deletes the file when you log out",
    ] {
        assert!(!flat.contains(phrase), "sidebar.md says {phrase:?}");
    }
    // It says the opposite, so the absence above is not an accident.
    assert!(flat.contains("the file stays on disk"));
}

#[test]
fn sidebar_md_names_the_menu_entries_the_app_has() {
    let flat = flat(&doc("sidebar.md"));
    for entry in ENTRIES {
        let label = entry.label().trim_end_matches('\u{2026}').to_lowercase();
        assert!(flat.contains(&label), "{label:?} is not in sidebar.md");
    }
}

#[test]
fn sidebar_md_states_the_caps_the_core_enforces() {
    let flat = flat(&doc("sidebar.md"));
    let grouped = |n: usize| {
        if n >= 1000 {
            format!("{},{:03}", n / 1000, n % 1000)
        } else {
            n.to_string()
        }
    };
    for (what, n) in [
        ("entries on the rail", limits::MAX_RAIL_ENTRIES),
        ("of them folders", limits::MAX_FOLDERS),
        ("sections of your own", limits::MAX_CUSTOM_SECTIONS),
        ("rooms in its starred", limits::MAX_PLACED_ROOMS),
        ("room and space references", limits::MAX_ROOM_REFS),
    ] {
        let wanted = format!("{} {what}", grouped(n));
        let wanted_up_to = format!("up to {wanted}");
        let wanted_at_most = format!("at most {wanted}");
        // "Up to 200 entries on the rail", "at most 50 of them folders".
        assert!(
            flat.contains(&wanted_up_to) || flat.contains(&wanted_at_most),
            "sidebar.md does not give the cap {wanted:?}"
        );
    }
    let bytes = usize::try_from(limits::MAX_SIDEBAR_BYTES).expect("small");
    assert!(flat.contains(&format!("{} kib", bytes / 1024)));
    assert!(flat.contains(&format!("1 to {} characters", SidebarLabel::MAX_CHARS)));
}

#[test]
fn the_docs_do_not_promise_what_the_sidebar_does_not_do() {
    let flat = flat(&doc("sidebar.md"));
    // Not built: a sync, a menu at the pointer, a keyboard way, auto-scroll.
    for promise in [
        "keyboard shortcut for",
        "shortcut to arrange",
        "scrolls automatically",
        "auto-scrolls",
        "at the pointer",
    ] {
        assert!(!flat.contains(promise), "sidebar.md promises {promise:?}");
    }
    // What it says instead.
    assert!(flat.contains("no keyboard shortcut was added"));
    assert!(flat.contains("does not scroll the list"));
}
