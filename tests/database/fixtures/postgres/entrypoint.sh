#!/usr/bin/env bash
set -euo pipefail

tls_dir=/var/lib/postgresql/yuzora-test-tls
install -d -o postgres -g postgres -m 0700 "$tls_dir"
install -o postgres -g postgres -m 0644 \
  /yuzora-fixtures/postgres/tls/server.crt "$tls_dir/server.crt"
install -o postgres -g postgres -m 0600 \
  /yuzora-fixtures/postgres/tls/server.key "$tls_dir/server.key"

exec /usr/local/bin/docker-entrypoint.sh "$@"
