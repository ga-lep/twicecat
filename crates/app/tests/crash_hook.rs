//! The crash hook in a process of its own (it is process-wide): a panic with
//! sensitive words writes one scrubbed report, and a second panic writes no
//! second one (Phase 7 Task 10).

use twicecat_app::crash;

#[test]
fn a_panic_writes_one_scrubbed_report() {
    let dir = std::env::temp_dir().join(format!("twicecat-crash-hook-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    crash::set_gpu_backend(Some("NVIDIA · CUDA".into()));
    crash::install(dir.clone());
    for _ in 0..2 {
        let joined = std::thread::Builder::new()
            .name("sync @alice:example.org".into())
            .spawn(|| {
                panic!("no member @bob:example.org in !room:example.org, token syt_YWxpY2U_aBcDeFgHiJkLmNoPq_1a2B3c")
            })
            .unwrap()
            .join();
        assert!(joined.is_err());
    }
    let reports = crash::reports(&dir);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let text = crash::newest(&dir).unwrap().text;
    let _ = std::fs::remove_dir_all(&dir);
    for want in [
        concat!("Version: ", env!("CARGO_PKG_VERSION")),
        "GPU backend: NVIDIA · CUDA",
        "Thread: sync <id>",
        "Panic: no member <id> in <id>, token <token>",
        "At: crates/app/tests/crash_hook.rs:",
    ] {
        assert!(text.contains(want), "{want} in\n{text}");
    }
    for never in ["alice", "bob", "example.org", "syt_"] {
        assert!(!text.contains(never), "{never} in\n{text}");
    }
}
