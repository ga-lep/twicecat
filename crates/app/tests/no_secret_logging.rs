//! Grep-style check (plan Q16, Task 14): no code in `crates/app/src` hands a
//! secret-bearing field or type to a logging or printing macro.
//!
//! The app has no `tracing` dependency, and the only prints are in `main.rs`
//! (command-line errors) and in `#[cfg(test)]` timing lines. This test pins
//! that: every invocation of a printing or logging macro outside test code is
//! found, and its argument text may name none of the secret-bearing fields,
//! types or models listed in [`FORBIDDEN`]. It is a tripwire, not a proof: a
//! secret copied into a local with an innocent name would pass.
//!
//! Printing and logging macros, the panicking and asserting macros (`panic!`,
//! `assert!`, `assert_eq!`, `assert_ne!`, their `debug_` forms, `unreachable!`,
//! `todo!`, `unimplemented!`) and the message of `.expect(` are scanned. String,
//! raw string and character literals and comments (`//`, nested `/* */`) are
//! skipped by one shared lexer step in the `#[cfg(test)]` stripping, the item
//! scan and the argument scan, so a `)` or `{` inside one does not end a list
//! or an item, and `#[cfg(test)]` or a macro name inside a comment or a string
//! is not code.
//!
//! This is a tripwire for ordinary mistakes, not a guarantee. It matches names
//! as text, so deliberate bypasses pass: a space before `!` (`println !(..)`),
//! raw identifiers (`r#password`), whitespace around `.` or `(`
//! (`s .password`, `x.expect (..)`), type aliases and qualified calls
//! (`K::expose(..)`, `Option::expect(x, msg)`), `.security` on receivers other
//! than `self`, and macros that are not listed (`format_args!`, `write_fmt`, a
//! bare `event!`, `log!` or `span!` after a `use`, `#[tracing::instrument]`).
//!
//! Limits that remain: `format!` into a local that is printed later; direct
//! writes such as `io::stderr().write_all(..)`; a secret copied into a local
//! with an innocent name; `.unwrap()` and derived `Debug` output of a value
//! that is formatted elsewhere; macros defined in other crates or wrapped by an
//! app macro; code behind `#[cfg(test)]` is not scanned on purpose.

use std::fs;
use std::path::{Path, PathBuf};

/// Macros that write somewhere a person or a file can read.
const MACROS: [&str; 26] = [
    "println!",
    "print!",
    "eprintln!",
    "eprint!",
    "dbg!",
    "write!",
    "writeln!",
    "trace!",
    "debug!",
    "info!",
    "warn!",
    "error!",
    "tracing::event!",
    "log::",
    "panic!",
    "assert!",
    "assert_eq!",
    "assert_ne!",
    "debug_assert!",
    "debug_assert_eq!",
    "debug_assert_ne!",
    "unreachable!",
    "todo!",
    "unimplemented!",
    // The message of `Option::expect` and `Result::expect`.
    ".expect",
    ".expect_err",
];

/// Names that must not appear in the arguments of those macros: the fields
/// and types that hold the recovery key, typed inputs, passwords, the QR grid
/// and the verification and sessions state (plan Q15).
const FORBIDDEN: [&str; 42] = [
    "RecoveryKey",
    "Password",
    "RecoverySetup",
    "ReauthPrompt",
    "SecurityModel",
    "VerificationModel",
    "SessionsModel",
    "QrImage",
    "SasEmoji",
    ".expose(",
    ".restore_input",
    ".setup",
    ".reauth",
    ".copied",
    ".password",
    ".qr",
    "qr_handle",
    ".dark",
    ".emojis",
    ".phase",
    "self.security",
    "self.sessions",
    "self.verification",
    // Phase 5: media bytes and file names, notification text, keywords, and
    // the models that hold typed text (task 17).
    "MediaBytes",
    "MediaModel",
    "UploadModel",
    "SavePath",
    "Toast",
    "NotificationView",
    "SettingsModel",
    "SwitcherModel",
    ".preview",
    ".sender_name",
    ".room_name",
    ".filename",
    ".keywords",
    ".keyword_input",
    ".switcher",
    ".settings_ui",
    ".viewer",
    ".dropped",
    ".draft",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("source directory is readable") {
        let path = entry.expect("directory entry is readable").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The one lexer step shared by every scan below: if a comment, string, raw
/// string or character literal starts at `i`, the index just past it (a `//`
/// comment stops before its newline). Block comments nest. A character literal
/// is told from a lifetime by its closing quote.
fn skip_trivia(code: &str, i: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    match bytes[i] {
        b'/' if bytes.get(i + 1) == Some(&b'/') => {
            Some(code[i..].find('\n').map_or(bytes.len(), |n| i + n))
        }
        b'/' if bytes.get(i + 1) == Some(&b'*') => {
            let mut depth = 1usize;
            let mut j = i + 2;
            while j < bytes.len() && depth > 0 {
                if bytes[j..].starts_with(b"/*") {
                    depth += 1;
                    j += 2;
                } else if bytes[j..].starts_with(b"*/") {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            Some(j)
        }
        b'r' if matches!(bytes.get(i + 1), Some(b'"' | b'#')) => {
            let hashes = bytes[i + 1..].iter().take_while(|b| **b == b'#').count();
            if bytes.get(i + 1 + hashes) != Some(&b'"') {
                return None;
            }
            let close = format!("\"{}", "#".repeat(hashes));
            let body = i + 2 + hashes;
            Some(
                code[body..]
                    .find(&close)
                    .map_or(bytes.len(), |n| body + n + close.len()),
            )
        }
        b'"' => {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'"' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            Some((j + 1).min(bytes.len()))
        }
        b'\'' => {
            if bytes.get(i + 1) == Some(&b'\\') {
                // An escape: skip the backslash and the byte after it (this
                // covers `'\''`), then find the closing quote (`'\u{1F600}'`).
                let from = (i + 3).min(bytes.len());
                let n = bytes[from..].iter().take(10).position(|b| *b == b'\'')?;
                Some(from + n + 1)
            } else {
                let len = code[i + 1..].chars().next()?.len_utf8();
                (bytes.get(i + 1 + len) == Some(&b'\'')).then_some(i + len + 2)
            }
        }
        _ => None,
    }
}

/// Index just past the item that starts at `from`: up to the first `;` at
/// brace depth 0, or past the matching `}` of its first `{`. Comments and
/// literals are skipped, so a brace inside one does not count.
fn item_end(code: &str, from: usize) -> usize {
    let bytes = code.as_bytes();
    let mut i = from;
    let mut depth = 0usize;
    while i < bytes.len() {
        if let Some(next) = skip_trivia(code, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            b';' if depth == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// The source without its `#[cfg(test)]` items (test modules, test-only
/// `use`s and statics). The attribute is matched only in code, not inside a
/// comment or a string.
fn production_part(source: &str) -> String {
    const ATTR: &[u8] = b"#[cfg(test)]";
    let bytes = source.as_bytes();
    let mut kept = String::new();
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if let Some(next) = skip_trivia(source, i) {
            i = next;
        } else if bytes[i..].starts_with(ATTR) {
            kept.push_str(&source[copied..i]);
            i = item_end(source, i + ATTR.len());
            copied = i;
        } else {
            i += 1;
        }
    }
    kept.push_str(&source[copied..]);
    kept
}

/// The text between the parentheses (or brackets) after each macro name, for
/// names that appear in code (not in a comment or a string).
fn invocations<'a>(code: &'a str, name: &str) -> Vec<&'a str> {
    let bytes = code.as_bytes();
    let method = name.starts_with('.');
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(next) = skip_trivia(code, i) {
            i = next;
            continue;
        }
        if !bytes[i..].starts_with(name.as_bytes()) {
            i += 1;
            continue;
        }
        let start = i + name.len();
        // `name` must not be the tail of a longer identifier (`my_print!`).
        let tail_of_identifier = i > 0
            && (bytes[i - 1].is_ascii_alphanumeric()
                || bytes[i - 1] == b'_'
                || bytes[i - 1] >= 0x80);
        i = start;
        if tail_of_identifier && !name.ends_with("::") && !method {
            continue;
        }
        let rest = &code[start..];
        let Some(open) = rest.find(['(', '[', '{']) else {
            continue;
        };
        if !rest[..open].trim().is_empty() && name != "log::" {
            continue;
        }
        let mut depth = 0usize;
        let mut end = None;
        let rest_bytes = rest.as_bytes();
        let mut j = open;
        while j < rest_bytes.len() {
            if let Some(next) = skip_trivia(rest, j) {
                j = next;
                continue;
            }
            match rest_bytes[j] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(j);
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        if let Some(end) = end {
            found.push(&rest[open..end]);
        }
    }
    found
}

/// Every violation in `source`, as `macro(args)` text.
fn violations(source: &str) -> Vec<String> {
    let code = production_part(source);
    let code = code.as_str();
    let mut bad = Vec::new();
    for name in MACROS {
        for args in invocations(code, name) {
            if let Some(word) = FORBIDDEN.iter().find(|w| args.contains(**w)) {
                bad.push(format!("{name} with `{word}`: {}", args.trim()));
            }
        }
    }
    bad
}

#[test]
fn no_app_code_logs_or_prints_a_secret_bearing_field() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(files.len() > 10, "the source tree was not found");
    let mut bad = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file).expect("source file is readable");
        for v in violations(&source) {
            bad.push(format!("{}: {v}", file.display()));
        }
    }
    assert!(
        bad.is_empty(),
        "secret-bearing logging:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_scanner_flags_every_macro_and_ignores_test_code_and_comments() {
    let mut missed = Vec::new();
    for planted in [
        r#"fn f(s: &S) { println!("{}", s.password); }"#,
        r#"fn f(s: &S) { eprintln!("{:?}", self.security); }"#,
        r#"fn f(k: &RecoveryKey) { dbg!(k as &RecoveryKey); }"#,
        r#"fn f(m: &M) { tracing::info!(setup = ?m.setup, "x"); }"#,
        r#"fn f(m: &M) { debug!("{:?}", m.qr_handle()); }"#,
        // Phase 5: media bytes, notification text, file names, keywords.
        r#"fn f(b: &MediaBytes) { println!("{:?}", MediaBytes::from(b)); }"#,
        r#"fn f(v: &V) { println!("{}", v.preview); }"#,
        r#"fn f(t: &Toast) { eprintln!("{:?}", Toast::from(t)); }"#,
        r#"fn f(n: &NotificationView) { debug!("{:?}", n.sender_name); }"#,
        r#"fn f(c: &C) { warn!("{}", c.filename); }"#,
        r#"fn f(s: &S) { info!("{:?}", s.keywords); }"#,
        r#"fn f(s: &S) { info!("{:?}", s.keyword_input); }"#,
        r#"fn f(s: &S) { info!("{:?}", s.switcher); }"#,
        r#"fn f(s: &S) { info!("{:?}", s.draft); }"#,
        r#"fn f(s: &S) { info!("{:?}", s.settings_ui.name); }"#,
        r#"fn f(s: &S) { info!("{:?}", s.dropped); }"#,
        r#"fn f(s: &S) { info!("{:?}", s.viewer); }"#,
        r#"fn f(m: &M) { log::warn!("{}", m.restore_input.expose()); }"#,
        // A `)` inside a string literal must not end the argument list early.
        r#"fn f(s: &S) { eprintln!(") {}", s.password); }"#,
        r#"fn f(s: &S) { eprintln!("{}", ")"); println!("{}", s.password); }"#,
        r#"fn f(s: &S) { println!("{}{}", ')', s.password); }"#,
        r##"fn f(s: &S) { println!(r#")"#, s.password); }"##,
        // Panicking and asserting macros print their message and operands.
        r#"fn f(s: &S) { panic!("{}", s.password); }"#,
        r#"fn f(s: &S) { assert!(ok, "{}", s.password); }"#,
        r#"fn f(s: &S) { assert_eq!(1, 2, "{}", s.password); }"#,
        r#"fn f(s: &S) { assert_ne!(s.password, 2); }"#,
        r#"fn f(s: &S) { debug_assert!(ok, "{}", s.password); }"#,
        r#"fn f(s: &S) { debug_assert_eq!(s.password, 2); }"#,
        r#"fn f(s: &S) { debug_assert_ne!(s.password, 2); }"#,
        r#"fn f(s: &S) { unreachable!("{}", s.password); }"#,
        r#"fn f(s: &S) { todo!("{}", s.password); }"#,
        r#"fn f(s: &S) { unimplemented!("{}", s.password); }"#,
        r#"fn f(s: &S) { x().expect(&format!("{}", s.password)); }"#,
        r#"fn f(s: &S) { x()
            .expect(&s.password); }"#,
        // An escaped quote character literal closes after its second quote.
        r#"fn f(s: &S) { eprintln!("{}{}{}", '\'', ')', s.password); }"#,
        // Comments inside the arguments hide nothing.
        r#"fn f(s: &S) { println!("{}", /* ) */ s.password); }"#,
        "fn f(s: &S) { println!(\"{}\", // )\n s.password); }",
        r#"fn f(s: &S) { println!("{}", /* /* ) */ ) */ s.password); }"#,
        // A brace in a comment must not swallow the rest of the file.
        "#[cfg(test)] fn h() { /* { */ }\nfn f(s: &S) { println!(\"{}\", s.password); }",
        // `#[cfg(test)]` inside a comment, doc line or string removes nothing.
        "// #[cfg(test)]\nfn f(s: &S) { println!(\"{}\", s.password); }",
        "/// uses #[cfg(test)] here\nfn f(s: &S) { println!(\"{}\", s.password); }",
        "/* #[cfg(test)] */ fn f(s: &S) { println!(\"{}\", s.password); }",
        "const A: &str = \"#[cfg(test)]\"; fn f(s: &S) { println!(\"{}\", s.password); }",
    ] {
        if violations(planted).is_empty() {
            missed.push(planted);
        }
    }
    assert!(missed.is_empty(), "not flagged:\n{}", missed.join("\n--\n"));
    let fine = r#"
fn f(message: &str) { eprintln!("tessera: {message}"); }
fn g(x: Option<u8>) { x.expect("a fixed message"); assert!(x.is_some(), "fine"); panic!("fine ) text"); }
// println!("{}", s.password);
/* println!("{}", s.password); /* nested */ eprintln!("{}", s.password); */
const A: &str = "println!(s.password)";
#[cfg(test)]
mod tests { fn t() { println!("{:?}", self.security); } }
"#;
    assert!(violations(fine).is_empty(), "{:?}", violations(fine));
}
