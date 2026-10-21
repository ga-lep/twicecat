//! Helpers for the tests that scan the app's own sources for a pattern (a
//! widget built without the kit style, a theme built outside `theme.rs`).
//! Test code only.
//!
//! The scans work on a *masked* copy of the code ([`mask`]): comments and the
//! inside of string and character literals are blanked, so a `//` in
//! `"https://"`, a `(` in a message or a `Theme::custom` in a doc comment is
//! not mistaken for code.

use std::path::{Path, PathBuf};

fn blank(c: char) -> char {
    if c == '\n' { '\n' } else { ' ' }
}

/// `code` with comments and the inside of string, raw string and character
/// literals replaced by spaces (newlines kept), so every index and line number
/// is the same as in `code`. The quotes themselves stay.
pub fn mask(code: &str) -> String {
    mask_chars(&code.chars().collect::<Vec<_>>())
        .into_iter()
        .collect()
}

fn mask_chars(chars: &[char]) -> Vec<char> {
    let mut out = chars.to_vec();
    let blank_range = |out: &mut Vec<char>, from: usize, to: usize| {
        for c in &mut out[from..to.min(chars.len())] {
            *c = blank(*c);
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            let end = chars[i..]
                .iter()
                .position(|&c| c == '\n')
                .map_or(chars.len(), |p| i + p);
            blank_range(&mut out, i, end);
            i = end;
        } else if c == '/' && next == Some('*') {
            // Block comments nest.
            let (mut depth, mut j) = (1, i + 2);
            while j < chars.len() && depth > 0 {
                if chars[j] == '/' && chars.get(j + 1) == Some(&'*') {
                    depth += 1;
                    j += 2;
                } else if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            blank_range(&mut out, i, j);
            i = j;
        } else if c == 'r' && raw_string_start(chars, i).is_some() {
            let (hashes, open) = raw_string_start(chars, i).expect("checked");
            let mut j = open + 1;
            while j < chars.len() {
                if chars[j] == '"' && (1..=hashes).all(|k| chars.get(j + k) == Some(&'#')) {
                    break;
                }
                j += 1;
            }
            blank_range(&mut out, open + 1, j);
            i = (j + 1 + hashes).min(chars.len());
        } else if c == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                if chars[j] == '\\' {
                    j += 1;
                }
                j += 1;
            }
            blank_range(&mut out, i + 1, j);
            i = j + 1;
        } else if c == '\'' {
            // A character literal ('x', '\n'), not a lifetime ('a).
            if next == Some('\\') {
                let end = chars[i + 2..]
                    .iter()
                    .position(|&c| c == '\'')
                    .map_or(chars.len(), |p| i + 2 + p);
                blank_range(&mut out, i + 1, end);
                i = end + 1;
            } else if chars.get(i + 2) == Some(&'\'') {
                blank_range(&mut out, i + 1, i + 2);
                i += 3;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// If a raw string (`r"`, `r#"`, `br##"`) starts at the `r` at `at`: its number
/// of `#` and the index of the opening quote.
fn raw_string_start(chars: &[char], at: usize) -> Option<(usize, usize)> {
    if at > 0
        && is_ident(chars[at - 1])
        && !(chars[at - 1] == 'b' && (at < 2 || !is_ident(chars[at - 2])))
    {
        return None;
    }
    let mut j = at + 1;
    while chars.get(j) == Some(&'#') {
        j += 1;
    }
    (chars.get(j) == Some(&'"')).then_some((j - at - 1, j))
}

/// `source` without its test modules: each inline `#[cfg(test)] mod x { .. }`
/// (after any further attributes) is blanked with spaces, newlines kept, so
/// line numbers stay. What follows a test module is still production code and
/// is kept. A `#[cfg(test)]` on a `use`, a statement, a function or an
/// out-of-line `mod x;` hides nothing.
pub fn production_part(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let masked = mask_chars(&chars);
    let mut out = chars.clone();
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(
            masked
                .iter()
                .enumerate()
                .filter(|&(_, &c)| c == '\n')
                .map(|(i, _)| i + 1),
        )
        .collect();
    let line = |n: usize| -> String {
        let from = line_starts[n];
        let to = line_starts.get(n + 1).copied().unwrap_or(masked.len());
        masked[from..to]
            .iter()
            .collect::<String>()
            .trim()
            .to_owned()
    };
    let mut n = 0;
    while n < line_starts.len() {
        if line(n) != "#[cfg(test)]" {
            n += 1;
            continue;
        }
        let item =
            (n + 1..line_starts.len()).find(|&k| !line(k).is_empty() && !line(k).starts_with("#["));
        let Some(k) = item else { break };
        let text = line(k);
        let is_module = text
            .strip_prefix("pub ")
            .or_else(|| text.strip_prefix("pub(crate) "))
            .unwrap_or(&text)
            .starts_with("mod ");
        // An inline module has its brace on the item line; `mod x;` has none.
        if is_module && text.contains('{') {
            let line_end = line_starts.get(k + 1).copied().unwrap_or(masked.len());
            let open = masked[line_starts[k]..line_end]
                .iter()
                .position(|&c| c == '{')
                .map(|p| line_starts[k] + p)
                .expect("the line has a brace");
            let end = skip_group(&masked, open);
            for c in &mut out[line_starts[n]..end] {
                *c = blank(*c);
            }
            // Resume at the line after the module's closing brace.
            n = line_starts.partition_point(|&s| s < end);
        } else {
            n = k;
        }
    }
    out.into_iter().collect()
}

/// Every `.rs` file under the app's `src`, as its path and whole text.
pub fn sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in std::fs::read_dir(dir).expect("readable source dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("readable source");
                out.push((path, text));
            }
        }
    }
    let mut out = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    out
}

/// One call found by [`calls`].
#[derive(Debug)]
pub struct Call {
    /// 1-based line of the call's name.
    pub line: usize,
    /// The call and the method chain that follows it, as written.
    pub chain: String,
    /// The methods chained onto the call, in order, with the text of their
    /// arguments (`("style", "kit::checkbox_style")`). Nested calls inside
    /// the arguments are not listed.
    pub methods: Vec<(String, String)>,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The index just past the group that opens at `open` (a `(`, `[` or `{`) in
/// masked code.
fn skip_group(masked: &[char], open: usize) -> usize {
    let mut depth = 0usize;
    for (i, &c) in masked.iter().enumerate().skip(open) {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
    }
    masked.len()
}

/// The calls of the free function `name` in `code`: `name(...)` and every
/// `.method(...)` chained onto it. A longer name that ends with `name`
/// (`a_checkbox(`), a method (`.name(`) and a definition (`fn name(`) are not
/// calls of it. Comments and string contents are ignored. The function under
/// another name (`use .. as ..`) is not found: see [`renamed_imports`].
pub fn calls(code: &str, name: &str) -> Vec<Call> {
    let original: Vec<char> = code.chars().collect();
    let chars = mask_chars(&original);
    let pattern: Vec<char> = format!("{name}(").chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + pattern.len() <= chars.len() {
        if chars[i..i + pattern.len()] != pattern[..] {
            i += 1;
            continue;
        }
        let before: String = chars[..i].iter().rev().take(4).collect();
        let glued = before
            .chars()
            .next()
            .is_some_and(|c| is_ident(c) || c == '.');
        let defined = before.starts_with(" nf");
        if glued || defined {
            i += 1;
            continue;
        }
        let mut end = skip_group(&chars, i + pattern.len() - 1);
        let mut methods = Vec::new();
        loop {
            let mut at = end;
            while chars.get(at).is_some_and(|c| c.is_whitespace()) {
                at += 1;
            }
            if chars.get(at) != Some(&'.') {
                break;
            }
            at += 1;
            let name_start = at;
            while chars.get(at).is_some_and(|c| is_ident(*c)) {
                at += 1;
            }
            if chars.get(at) != Some(&'(') {
                break;
            }
            let name: String = chars[name_start..at].iter().collect();
            end = skip_group(&chars, at);
            let args: String = original[at + 1..end.saturating_sub(1).max(at + 1)]
                .iter()
                .collect();
            methods.push((name, args));
        }
        found.push(Call {
            line: chars[..i].iter().filter(|&&c| c == '\n').count() + 1,
            chain: original[i..end].iter().collect(),
            methods,
        });
        i = end.max(i + 1);
    }
    found
}

/// The `use` renames of any of `names` in `code`: `checkbox as cb`. A call
/// through the new name would escape [`calls`], so a scan that must not miss
/// one flags the rename itself. Each hit is the 1-based line and the name.
pub fn renamed_imports(code: &str, names: &[&str]) -> Vec<(usize, String)> {
    let chars = mask_chars(&code.chars().collect::<Vec<_>>());
    let mut hits = Vec::new();
    for name in names {
        let pattern: Vec<char> = name.chars().collect();
        for i in 0..chars.len().saturating_sub(pattern.len() - 1) {
            if chars[i..i + pattern.len()] != pattern[..] {
                continue;
            }
            if i > 0 && is_ident(chars[i - 1]) {
                continue;
            }
            let mut at = i + pattern.len();
            if chars.get(at).is_some_and(|&c| is_ident(c)) {
                continue;
            }
            let space_from = at;
            while chars.get(at).is_some_and(|c| c.is_whitespace()) {
                at += 1;
            }
            let renamed = at > space_from
                && chars.get(at) == Some(&'a')
                && chars.get(at + 1) == Some(&'s')
                && chars.get(at + 2).is_some_and(|c| c.is_whitespace());
            if renamed {
                let line = chars[..i].iter().filter(|&&c| c == '\n').count() + 1;
                hits.push((line, (*name).to_owned()));
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_production_cut_is_at_the_test_modules() {
        let source = "fn a() {}\n#[cfg(test)]\nuse std::cell::Cell;\nfn b() {}\n\
                      #[cfg(test)]\n#[allow(dead_code)]\nmod fake {\n}\nfn c() {}\n\
                      #[cfg(test)]\nmod tests {\n    fn t() {}\n}\n";
        let code = production_part(source);
        assert!(code.contains("fn b()"), "a cfg(test) use hides code");
        assert!(code.contains("fn c()"), "code after a test module is kept");
        assert!(!code.contains("fake") && !code.contains("fn t()"));
        assert_eq!(code.lines().count(), source.lines().count(), "lines kept");
        let plain = "fn a() {}\n#[cfg(test)]\npub mod test_support {}\nfn z() {}\n";
        let code = production_part(plain);
        assert!(!code.contains("test_support") && code.contains("fn z()"));
        assert_eq!(production_part("fn only() {}"), "fn only() {}");
    }

    #[test]
    fn an_out_of_line_test_module_hides_nothing() {
        // `lib.rs` has `#[cfg(test)] mod source_scan;` before more modules.
        let source = "mod a;\n#[cfg(test)]\nmod source_scan;\nmod b;\nfn later() {}\n\
                      #[cfg(test)]\nmod tests {\n    fn t() { let s = \"}\"; }\n}\nfn last() {}\n";
        let code = production_part(source);
        assert!(code.contains("mod b;") && code.contains("fn later()"));
        assert!(code.contains("fn last()"), "kept after the inline module");
        assert!(!code.contains("fn t()"));
    }

    #[test]
    fn masking_blanks_comments_and_literals_but_keeps_the_layout() {
        let code = "let u = \"https://x\"; // note ( \nlet r = r#\"a \" // b\"#; /* ( /* ) */ ) */ let c = '\"'; fn f<'a>() {}\n";
        let masked = mask(code);
        assert_eq!(masked.chars().count(), code.chars().count());
        assert_eq!(masked.matches('\n').count(), 2);
        for gone in ["https", "note", "// b", "/*", "'\"'"] {
            assert!(!masked.contains(gone), "{gone} survived: {masked}");
        }
        assert!(masked.contains("let u =") && masked.contains("fn f<'a>() {}"));
        assert_eq!(masked.matches('(').count(), 1, "only the fn's paren");
    }

    #[test]
    fn calls_are_found_with_their_whole_method_chain() {
        let code = "fn checkbox(x: u8) {}\n\
                    let a = checkbox(on)\n    .label(\"a (b) )\")\n    .style(kit::checkbox_style);\n\
                    let b = a_checkbox(on).style(x);\n\
                    let c = w.checkbox(on);\n\
                    // checkbox(commented)\n\
                    let d = checkbox('x').on_toggle(|t| t);\n";
        let found = calls(code, "checkbox");
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].line, 2);
        assert!(found[0].chain.ends_with(".style(kit::checkbox_style)"));
        assert_eq!(found[0].methods.len(), 2);
        assert_eq!(
            found[0].methods[1],
            ("style".to_owned(), "kit::checkbox_style".to_owned())
        );
        assert_eq!(found[1].line, 8);
        assert!(!found[1].chain.contains("style"));
    }

    #[test]
    fn a_double_slash_inside_a_string_is_not_a_comment() {
        let code = "let a = [\"https://x\", checkbox(on)];\n\
                    let b = r#\"\" // \"#; let c = checkbox(off).style(k);\n\
                    /* checkbox(in_block) */ let d = checkbox(on3);\n\
                    let e = \"checkbox(in_string)\";\n";
        let found = calls(code, "checkbox");
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(found.iter().map(|c| c.line).collect::<Vec<_>>(), [1, 2, 3]);
    }

    #[test]
    fn a_raw_string_or_block_comment_inside_a_call_does_not_end_it() {
        let code = "let a = checkbox(on)\n    .label(r#\"a ) \" ) \"#)\n    /* ) */\n    .style(kit::checkbox_style);\n";
        let found = calls(code, "checkbox");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].methods.len(), 2, "{:?}", found[0].methods);
    }

    #[test]
    fn renamed_imports_are_found() {
        let code = "use iced::widget::checkbox as cb;\n\
                    use iced::widget::{text_input as ti, text_editor\n    as te, button};\n\
                    use iced::widget::checkbox;\n\
                    // use iced::widget::checkbox as nope;\n\
                    let a_checkbox_as = 1; let checkboxes = 2;\n";
        let hits = renamed_imports(code, &["checkbox", "text_input", "text_editor"]);
        let mut names: Vec<_> = hits.iter().map(|(l, n)| (*l, n.as_str())).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [(1, "checkbox"), (2, "text_editor"), (2, "text_input")]
        );
    }
}
