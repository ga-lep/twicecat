Task 1: started
Task 1: complete (05193b7..fda97df, gate ok, security PASS, review PASS)
  notes: [sec Low] no RustSec advisory scan yet (cargo-deny lands in Task 3); [review Warn] PROGRESS.md doesn't cite where iced::widget::space was confirmed (iced_widget-0.14.2/src/helpers.rs); [review Sugg] TITLE constant test is low value
Task 2: started
Task 2: complete (eeb37aa..4808065, gate ok, security PASS, review PASS)
  notes: [sec Low] passwords visible in ps via curl -d (use --data-binary @- on stdin); [sec Low] JSON built by string formatting (jq or charset check); [sec Low] rc_registration relaxed + open reg — keep 127.0.0.1 binding, add comment; [sec Low] synapse image pinned by tag not digest (human decision, rule 3); [review Sugg] session parsed with sed — fine for Synapse
Task 3: started
Task 3: complete (c9d934b..0611914, gate ok, security PASS, review PASS)
  notes: [sec Med] check-app-deps.sh fails open if cargo tree errors (pipeline in if-condition); [sec Med] boundary check misses optional/target-specific deps (add --all-features --target all); [sec Med] ci.yml has no `permissions: contents: read`; [sec Med] actions/checkout without persist-credentials: false; [sec Low] actions pinned by tag not SHA; [sec Low] no --locked on cargo in CI; [review Warn] pattern is a regex; [review Sugg] cargo deny --all-features; advisories RUSTSEC-2024-0436 (paste), RUSTSEC-2026-0192 (ttf-parser) ignored as unmaintained via iced 0.14.0
