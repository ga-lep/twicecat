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
//! raw string and character literals in the arguments are skipped, so a `)`
//! inside one does not end the argument list.
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
const FORBIDDEN: [&str; 23] = [
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

/// If a string, raw string or character literal starts at `i`, the index just
/// past it. A character literal closes within four bytes; a lifetime does not.
fn skip_literal(code: &str, i: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    match bytes[i] {
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
            let n = code[i + 1..].bytes().take(4).position(|b| b == b'\'')?;
            (bytes.get(i + 1) != Some(&b'\\') || n > 0).then_some(i + n + 2)
        }
        _ => None,
    }
}

/// Index just past the item that starts at `from`: up to the first `;` at
/// brace depth 0, or past the matching `}` of its first `{`. Skips comments,
/// string and raw string literals and character literals, so a brace inside
/// one does not count.
fn item_end(code: &str, from: usize) -> usize {
    let bytes = code.as_bytes();
    let mut i = from;
    let mut depth = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if let Some(next) = skip_literal(code, i) {
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
/// `use`s and statics).
fn production_part(source: &str) -> String {
    const ATTR: &str = "#[cfg(test)]";
    let mut kept = String::new();
    let mut rest = source;
    while let Some(at) = rest.find(ATTR) {
        kept.push_str(&rest[..at]);
        let after = at + ATTR.len();
        let end = item_end(rest, after);
        rest = &rest[end..];
    }
    kept.push_str(rest);
    kept
}

/// The text between the parentheses (or brackets) after each macro name.
fn invocations<'a>(code: &'a str, name: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = code[from..].find(name) {
        let start = from + at + name.len();
        from = start;
        // Skip comments and doc lines: only code that is not behind `//`.
        let line_start = code[..start].rfind('\n').map_or(0, |n| n + 1);
        if code[line_start..start].trim_start().starts_with("//") {
            continue;
        }
        // `name` must not be the tail of a longer identifier (`my_print!`).
        let before = code[..from - name.len()].chars().next_back();
        let method = name.starts_with('.');
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_')
            && !name.ends_with("::")
            && !method
        {
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
        let bytes = rest.as_bytes();
        let mut i = open;
        while i < bytes.len() {
            if let Some(next) = skip_literal(rest, i) {
                i = next;
                continue;
            }
            match bytes[i] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
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
    for planted in [
        r#"fn f(s: &S) { println!("{}", s.password); }"#,
        r#"fn f(s: &S) { eprintln!("{:?}", self.security); }"#,
        r#"fn f(k: &RecoveryKey) { dbg!(k as &RecoveryKey); }"#,
        r#"fn f(m: &M) { tracing::info!(setup = ?m.setup, "x"); }"#,
        r#"fn f(m: &M) { debug!("{:?}", m.qr_handle()); }"#,
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
    ] {
        assert!(!violations(planted).is_empty(), "not flagged: {planted}");
    }
    let fine = r#"
fn f(message: &str) { eprintln!("tessera: {message}"); }
fn g(x: Option<u8>) { x.expect("a fixed message"); assert!(x.is_some(), "fine"); panic!("fine ) text"); }
// println!("{}", s.password);
#[cfg(test)]
mod tests { fn t() { println!("{:?}", self.security); } }
"#;
    assert!(violations(fine).is_empty(), "{:?}", violations(fine));
}
