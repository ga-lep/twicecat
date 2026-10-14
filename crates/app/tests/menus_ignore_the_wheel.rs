//! Every drop-down in the app must go through `ui::kit::menu`.
//!
//! Ctrl + wheel is the zoom gesture, but a closed iced `pick_list` takes it
//! as "select the next option": over the microphone menu or the "Send" mode
//! of the voice settings that changes what the user transmits. `kit::menu`
//! hides wheel events from the list, so a `pick_list(` anywhere else in the
//! app is a bug.

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

#[test]
fn every_pick_list_is_wrapped_by_the_wheel_guard() {
    let mut files = Vec::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(files.len() > 20, "found the sources");
    let mut bare = Vec::new();
    let mut seen = 0;
    for (name, text) in &files {
        // Comments may talk about `pick_list(`.
        let code: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for (at, _) in code.match_indices("pick_list(") {
            let before = &code[..at];
            // A path such as `iced::widget::pick_list(` is still the call.
            let before =
                before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == ':');
            let before = before.trim_end();
            seen += 1;
            if !(before.ends_with("menu(") || before.ends_with("no_wheel(")) {
                let line = code[..at].lines().count();
                bare.push(format!("{name}:{line}"));
            }
        }
    }
    assert!(seen >= 14, "the scan saw {seen} menus; did it break?");
    assert!(
        bare.is_empty(),
        "pick_list without kit::menu (Ctrl + wheel would change it): {bare:?}"
    );
}
