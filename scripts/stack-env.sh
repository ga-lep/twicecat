# shellcheck shell=bash
# Sourced by the stack and test scripts. load_stack_env exports the local
# test stack's settings: each from the environment if set there, else from
# .env.local (this machine's own settings, not committed), else from
# .env.test (the committed defaults). Then it derives the URLs the clients
# use from the address and ports (HOMESERVER_URL, LK_JWT_URL, LIVEKIT_URL and
# their _B twins), unless the environment sets them. The Rust tests read the
# same files in the same order (crates/core/tests/common/stack_env.rs).
# Never prints values: .env.test holds the test accounts' passwords.

# Exports KEY=VALUE lines of $1 whose key is not set yet. `#` comments and
# blank lines are skipped; spaces around the key and the value are trimmed,
# and values are otherwise taken verbatim (no quotes, no expansion), as the
# Rust tests read them.
# $1 without leading and trailing whitespace.
trim() {
    local s=$1
    s=${s#"${s%%[![:space:]]*}"}
    printf '%s' "${s%"${s##*[![:space:]]}"}"
}

load_env_file() {
    local file=$1 line key value
    [ -f "$file" ] || return 0
    while IFS= read -r line || [ -n "$line" ]; do
        line=${line%$'\r'}
        line=$(trim "$line")
        case "$line" in '' | '#'*) continue ;; esac
        [[ "$line" == *=* ]] || continue
        key=$(trim "${line%%=*}")
        value=$(trim "${line#*=}")
        [[ "$key" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || continue
        if [ -z "${!key+set}" ]; then
            export "$key=$value"
        fi
    done <"$file"
}

load_stack_env() {
    local root
    root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
    [ -f "$root/.env.test" ] || { echo "error: $root/.env.test missing" >&2; return 1; }
    load_env_file "$root/.env.local"
    load_env_file "$root/.env.test"
    local host=$TESSERA_ADDRESS
    # An IPv6 address goes in brackets in URLs and in published ports.
    [[ "$host" == *:* ]] && host="[$host]"
    export TESSERA_PUBLISH_ADDRESS="$host"
    : "${HOMESERVER_URL:=http://$host:$TESSERA_SYNAPSE_PORT}"
    : "${LK_JWT_URL:=http://$host:$TESSERA_LK_JWT_PORT}"
    : "${LIVEKIT_URL:=ws://$host:$TESSERA_LIVEKIT_PORT}"
    : "${LK_JWT_URL_B:=http://$host:$TESSERA_LK_JWT_PORT_B}"
    : "${LIVEKIT_URL_B:=ws://$host:$TESSERA_LIVEKIT_PORT_B}"
    export HOMESERVER_URL LK_JWT_URL LIVEKIT_URL LK_JWT_URL_B LIVEKIT_URL_B
}
