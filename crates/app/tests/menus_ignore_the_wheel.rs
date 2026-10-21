//! Every drop-down in the app must go through `ui::kit::menu`.
//!
//! Ctrl + wheel is the zoom gesture, but a closed iced `pick_list` takes it
//! as "select the next option": over the microphone menu or the "Send" mode
//! of the voice settings that changes what the user transmits. `kit::menu`
//! hides wheel events from the list, so a `pick_list(` anywhere else in the
//! app is a bug. The same goes for a `slider(` or a `vertical_slider(`: iced's
//! slider takes Ctrl + wheel as a step, which would move the microphone gain
//! or a volume while the user only zooms; `ui::guard::no_wheel` hides it.

use std::fs;
use std::path::Path;

fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(dir).expect("a readable source dir") {
        let path = entry.expect("a dir entry").path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = fs::read_to_string(&path).expect("a readable source file");
            out.push((path.display().to_string(), text));
        }
    }
}

/// The code of a source file without its comment lines.
fn code_of(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The callee that wraps the widget call at byte `at`: the path before the
/// `(` that opens the argument the widget is in, exactly as written (so
/// `kit::menu`, not `x_menu`), or `None` when the widget is not an argument.
fn wrapper_at(code: &str, at: usize) -> Option<String> {
    let before = &code[..at];
    // A path such as `iced::widget::pick_list(` is still the call.
    let before = before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == ':');
    let before = before.trim_end();
    let before = before.strip_suffix('(')?;
    let start = before
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
        .map_or(0, |i| i + 1);
    Some(before[start..].to_owned())
}

/// Every call of `widget(` outside comments, as `(file, line, wrapper)`,
/// not counting longer names that end in it (`a_slider(`) or its
/// definition.
fn calls_of(files: &[(String, String)], widget: &str) -> Vec<(String, usize, Option<String>)> {
    let mut out = Vec::new();
    for (name, text) in files {
        let code = code_of(text);
        for (at, _) in code.match_indices(&format!("{widget}(")) {
            // `a_slider(`, `vertical_slider(`: another name. A path
            // (`iced::widget::slider(`) is still the widget.
            if code[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            if code[..at].trim_end().ends_with("fn") {
                continue;
            }
            let line = code[..at].lines().count();
            out.push((name.clone(), line, wrapper_at(&code, at)));
        }
    }
    out
}

fn app_sources() -> Vec<(String, String)> {
    let mut files = Vec::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(files.len() > 20, "found the sources");
    files
}

const MENU_WRAPPERS: [&str; 6] = [
    "kit::menu",
    "ui::kit::menu",
    "crate::ui::kit::menu",
    "no_wheel",
    "guard::no_wheel",
    "crate::ui::guard::no_wheel",
];

fn unwrapped(files: &[(String, String)], widget: &str) -> (usize, Vec<String>) {
    let calls = calls_of(files, widget);
    let bare = calls
        .iter()
        .filter(|(_, _, wrapper)| {
            !wrapper
                .as_deref()
                .is_some_and(|w| MENU_WRAPPERS.contains(&w))
        })
        .map(|(file, line, wrapper)| format!("{file}:{line} (inside {wrapper:?})"))
        .collect();
    (calls.len(), bare)
}

#[test]
fn every_pick_list_is_wrapped_by_the_wheel_guard() {
    let (seen, bare) = unwrapped(&app_sources(), "pick_list");
    assert!(seen >= 14, "the scan saw {seen} menus; did it break?");
    assert!(
        bare.is_empty(),
        "pick_list without kit::menu (Ctrl + wheel would change it): {bare:?}"
    );
}

#[test]
fn the_sidebar_cards_lists_are_pick_lists_behind_the_wheel_guard() {
    let own: Vec<(String, String)> = app_sources()
        .into_iter()
        .filter(|(name, _)| name.ends_with("screens/sidebar_menu.rs"))
        .collect();
    let (seen, bare) = unwrapped(&own, "pick_list");
    assert_eq!(
        seen, 2,
        "the Move to folder and the Move to section lists are in the scan"
    );
    assert!(bare.is_empty(), "unwrapped: {bare:?}");
    // And the second one is the section list, wrapped on its own: a card that
    // moved a room into a section on Ctrl + wheel would rearrange the sidebar.
    let code = code_of(&own[0].1);
    let at = code.find("\"Move to section\"").expect("the section list");
    let wrapped = code[at..]
        .find("kit::menu(")
        .zip(code[at..].find("pick_list("))
        .is_some_and(|(menu, list)| menu < list);
    assert!(wrapped, "the Move to section list is not inside kit::menu");
}

#[test]
fn every_slider_is_wrapped_by_the_wheel_guard() {
    let files = app_sources();
    for widget in ["slider", "vertical_slider"] {
        let (seen, bare) = unwrapped(&files, widget);
        if widget == "slider" {
            assert!(seen >= 1, "the scan saw {seen} sliders; did it break?");
        }
        assert!(
            bare.is_empty(),
            "{widget} without no_wheel (Ctrl + wheel would move it): {bare:?}"
        );
    }
}

#[test]
fn the_scan_tells_a_wrapped_widget_from_a_bare_one() {
    let sample = |text: &str| vec![("sample.rs".to_owned(), text.to_owned())];
    let bare = |text: &str, widget: &str| unwrapped(&sample(text), widget).1.len();
    // Wrapped exactly.
    assert_eq!(bare("kit::menu(pick_list(a, b, c))", "pick_list"), 0);
    assert_eq!(
        bare("kit::menu(\n  iced::widget::pick_list(a))", "pick_list"),
        0
    );
    assert_eq!(bare("no_wheel(slider(0..=1, v, f))", "slider"), 0);
    assert_eq!(
        bare(
            "crate::ui::guard::no_wheel(\n slider(0..=1, v, f))",
            "slider"
        ),
        0
    );
    // Bare, or wrapped by something that is not the guard.
    assert_eq!(bare("pick_list(a, b, c)", "pick_list"), 1);
    assert_eq!(bare("column![pick_list(a, b, c)]", "pick_list"), 1);
    assert_eq!(bare("device_menu(pick_list(a, b, c))", "pick_list"), 1);
    assert_eq!(bare("my_no_wheel(slider(0..=1, v, f))", "slider"), 1);
    assert_eq!(bare("slider(0..=1, v, f)", "slider"), 1);
    assert_eq!(
        bare("x.push(vertical_slider(0..=1, v, f))", "vertical_slider"),
        1
    );
    // Another name that ends in the widget's is not the widget.
    assert_eq!(bare("a_slider(ui, settings)", "slider"), 0);
    assert_eq!(bare("fn limit_slider(ui: u8)", "slider"), 0);
    assert_eq!(bare("// slider(0..=1, v, f)", "slider"), 0);
}
