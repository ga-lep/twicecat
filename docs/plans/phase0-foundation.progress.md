Task 1: started
Task 1: complete (05193b7..fda97df, gate ok, security PASS, review PASS)
  notes: [sec Low] no RustSec advisory scan yet (cargo-deny lands in Task 3); [review Warn] PROGRESS.md doesn't cite where iced::widget::space was confirmed (iced_widget-0.14.2/src/helpers.rs); [review Sugg] TITLE constant test is low value
Task 2: started
Task 2: complete (eeb37aa..4808065, gate ok, security PASS, review PASS)
  notes: [sec Low] passwords visible in ps via curl -d (use --data-binary @- on stdin); [sec Low] JSON built by string formatting (jq or charset check); [sec Low] rc_registration relaxed + open reg — keep 127.0.0.1 binding, add comment; [sec Low] synapse image pinned by tag not digest (human decision, rule 3); [review Sugg] session parsed with sed — fine for Synapse
