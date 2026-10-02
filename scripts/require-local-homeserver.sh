# shellcheck shell=bash
# Sourced by the test scripts. Refuses (exit 1) any HOMESERVER_URL that is not
# plain http on a loopback address and a port, so that the test accounts'
# passwords are never sent to a real server (AGENTS.md rule 6).
require_local_homeserver() {
    local url=${HOMESERVER_URL:-}
    if [[ ! $url =~ ^http://(127\.0\.0\.1|localhost|\[::1\]):[0-9]+/?$ ]]; then
        echo "refusing to use HOMESERVER_URL: it must be http://127.0.0.1:<port>, http://localhost:<port> or http://[::1]:<port> (test accounts are local only)" >&2
        exit 1
    fi
}
