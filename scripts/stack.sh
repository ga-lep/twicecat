#!/usr/bin/env bash
# Runs docker compose for the local test stack (docker-compose.yml) with its
# address and ports: from the environment, else .env.local, else .env.test
# (see scripts/stack-env.sh). docker-compose.yml refuses to start without
# them, so use this instead of a bare `docker compose`.
#
# Usage: scripts/stack.sh up -d --wait | down -v | ps | logs <service> | ...
set -euo pipefail
script_dir=$(cd "$(dirname "$0")" && pwd)
cd "$script_dir/.."
# shellcheck source=scripts/stack-env.sh
source "$script_dir/stack-env.sh"
load_stack_env
# The tests accept 127.0.0.1 and ::1 only (crates/core/tests/common).
case "$TESSERA_ADDRESS" in
    127.0.0.1) ;;
    ::1)
        echo "warning: TESSERA_ADDRESS=::1 is untested: lk-jwt-service reaches LiveKit at this" \
            "address from its container, which needs IPv6 in Docker." >&2
        ;;
    *)
        echo "warning: TESSERA_ADDRESS=$TESSERA_ADDRESS is not 127.0.0.1 or ::1: this Synapse has" \
            "open registration and throwaway test accounts, and the tests refuse it (AGENTS.md rule 6)." >&2
        ;;
esac
exec docker compose "$@"
