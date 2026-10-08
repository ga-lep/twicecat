# libwebrtc (fetched, never committed)

`.cargo/config.toml` points `LK_CUSTOM_WEBRTC` here. `scripts/fetch-libwebrtc.sh`
downloads LiveKit's prebuilt libwebrtc, checks it against the SHA-256 in
`scripts/libwebrtc.sha256`, unpacks it into the user cache, and links its
entries (`include`, `lib`, `webrtc.ninja`, ...) into this directory.

This directory must always exist. webrtc-sys downloads libwebrtc, unverified,
into `LK_CUSTOM_WEBRTC` whenever that path is missing (webrtc-sys 0.3.47
build.rs:112). With the directory present but not fetched, the build fails on
the missing `webrtc.ninja` instead.

Windows: symlinks need extra privileges, so set `LK_CUSTOM_WEBRTC` to the path
the script prints (an environment variable takes precedence over the config).
