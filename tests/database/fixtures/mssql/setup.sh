#!/usr/bin/env bash
set -euo pipefail

if [[ -x /opt/mssql-tools18/bin/sqlcmd ]]; then
  sqlcmd=/opt/mssql-tools18/bin/sqlcmd
elif [[ -x /opt/mssql-tools/bin/sqlcmd ]]; then
  sqlcmd=/opt/mssql-tools/bin/sqlcmd
else
  exit 1
fi

export SQLCMDPASSWORD="$MSSQL_SA_PASSWORD"
tls_args=(-C)
ready=0
for _ in $(seq 1 60); do
  if "$sqlcmd" -S mssql -U sa "${tls_args[@]}" -Q "SELECT 1" -b -o /dev/null 2>&1; then
    ready=1
    break
  fi
  if "$sqlcmd" -S mssql -U sa -Q "SELECT 1" -b -o /dev/null 2>&1; then
    tls_args=()
    ready=1
    break
  fi
  sleep 2
done

[[ "$ready" -eq 1 ]]
"$sqlcmd" -S mssql -U sa "${tls_args[@]}" -b \
  -i /yuzora-fixtures/mssql/init.sql -o /dev/null
"$sqlcmd" -S mssql -U sa "${tls_args[@]}" -d yuzora_p8 \
  -Q "SET NOCOUNT ON; IF (SELECT COUNT(*) FROM alpha.rows_1201) <> 1201 THROW 51000, 'fixture validation failed', 1;" \
  -b -o /dev/null

touch /tmp/yuzora-mssql-ready
exec tail -f /dev/null
