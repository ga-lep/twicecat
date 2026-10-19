# Security

## Reporting a vulnerability

Report it privately: on the repository's [Security tab](https://github.com/ga-lep/twicecat/security), choose **Report a vulnerability**. Please don't open a public issue, pull request or discussion for it.

Say what you found, the version or commit, the OS, and how to reproduce it. Leave real access tokens, recovery keys, passwords and message contents out of the report; a made-up or redacted value shows the problem just as well.

Twicecat is in early development, so only the latest release and `main` get fixes.

## What counts

Anything that lets someone read or change what they should not, for example:

- a token, key, password, recovery key or message body in a log, a crash report or an error message;
- remote content (a message, a name, an avatar, a file, a link) that runs code, crashes the app or opens something without the user asking;
- an encrypted room or call that is not encrypted end to end when Twicecat says it is;
- a download that is not checked against its pinned SHA-256 (`scripts/fetch-*.sh`, `scripts/checksums/`).
