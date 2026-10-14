# Evidence — Task 17: Apply the default colour set the human picked
Commit: 2308f03
Environment: headless; CC=clang-22 CXX=clang++-22, repo's own target dir, .cargo/config.toml untouched. Gallery PNGs written to the session scratchpad (not in repo).

## DEFAULT_FAMILY = Frost (one-line change); palette None -> Frost
Status: PROVEN
```console
$ grep -n "DEFAULT_FAMILY: Family" crates/app/src/theme.rs
283:pub const DEFAULT_FAMILY: Family = Family::Frost;
$ cargo test -p twicecat-app --lib theme::
running 15 tests
test theme::tests::no_palette_resolves_to_the_default_family ... ok
test theme::tests::only_theme_rs_names_the_default_family ... ok
(... 13 more ok)
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 1224 filtered out
```
The test asserts resolve_family(None) == Frost and theme_for(Dark) is "Frost Dark". The src diff in crates/ is that line plus test expectations.

## Explicit Twicecat still Twicecat; swatch marked "(default)" is Frost
Status: PROVEN
```console
$ cargo test -p twicecat-app --test ui_gallery -- the_swatch_marked_default_is_frost a_settings_file_with_an_explicit_twicecat_palette_still_shows_twicecat
test settings_pages::a_settings_file_with_an_explicit_twicecat_palette_still_shows_twicecat ... ok
test settings_pages::the_swatch_marked_default_is_frost ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 122 filtered out
```
Visual check (TWICECAT_GALLERY_ONLY=settings-appearance, `-- --ignored`, PNGs inspected with Read): in settings-appearance-frost-dark-comfortable.png the Frost swatch reads "Frost (default)" (selected, teal); in settings-appearance-twicecat-light-comfortable.png the Twicecat swatch is selected and the label "Frost (default)" sits on the Frost swatch, not Twicecat.
Login (TWICECAT_GALLERY_ONLY=login): login-frost-dark-comfortable.png has a teal Log in button and teal-grey surfaces; login-twicecat-dark-comfortable.png has a periwinkle button and neutral grey. Note: the gallery draws each family explicitly, so the "no palette set -> Frost look" is proven by the unit test above (theme_for(Dark)=="Frost Dark"), not by a gallery screen.

## Mutation: DEFAULT_FAMILY back to Twicecat -> new tests fail
Status: PROVEN
```console
$ sed -i ... DEFAULT_FAMILY = Family::Twicecat   (original copied beforehand)
test theme::tests::no_palette_resolves_to_the_default_family ... FAILED
test result: FAILED. 14 passed; 1 failed
test settings_pages::a_settings_file_with_an_explicit_twicecat_palette_still_shows_twicecat ... FAILED
test settings_pages::the_swatch_marked_default_is_frost ... FAILED
test result: FAILED. 0 passed; 2 failed
$ cp <backup> crates/app/src/theme.rs; git status --short
(empty)
```

## Evidence hint: run the app with a fresh data dir, opens in Frost, Appearance marks "(default)"
Status: NOT PROVEN (human — GUI cannot be driven headlessly; the rendered gallery PNGs above stand in)

## Other criteria (PROGRESS.md records pick/date, commit message, human sign-off)
Status: NOT PROVEN (not outside-observable here; docs/process)

## Guard
```console
$ scripts/check-no-webrtc-download.sh; echo exit=$?
exit=0
```
