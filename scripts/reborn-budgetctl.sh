#!/usr/bin/env bash
# Inspect or reset a Reborn instance budget through the native governor API.
set -euo pipefail

usage() {
  cat <<'USAGE'
usage:
  reborn-budgetctl.sh status <instance>
  reborn-budgetctl.sh reset-period <instance> --confirm-reset
USAGE
}

[[ $# -ge 2 ]] || { usage >&2; exit 2; }
ACTION="$1"
INSTANCE_ID="$2"
CONFIRM="${3:-}"

[[ "$INSTANCE_ID" =~ ^[a-zA-Z0-9][a-zA-Z0-9_-]*$ ]] || {
  echo "ERROR: invalid instance id: $INSTANCE_ID" >&2
  exit 2
}
ENV_FILE="/etc/ironclaw/instances/${INSTANCE_ID}.env"
UNIT="ironclaw-reborn@${INSTANCE_ID}.service"
BINARY="${IRONCLAW_REBORN_BIN:-/opt/ironclaw/src/target/release/ironclaw-reborn}"

[[ -x "$BINARY" ]] || { echo "ERROR: budget-capable binary not found: $BINARY" >&2; exit 1; }

MAIN_PID="$(systemctl show --property MainPID --value "$UNIT")"
[[ "$MAIN_PID" =~ ^[1-9][0-9]*$ ]] || {
  echo "ERROR: $UNIT has no running main process" >&2
  exit 1
}

env_value() {
  if [[ -r "$ENV_FILE" ]]; then
    sed -n "s/^$1=//p" "$ENV_FILE" | tail -n 1
  else
    tr '\0' '\n' < "/proc/${MAIN_PID}/environ" | sed -n "s/^$1=//p" | tail -n 1
  fi
}

HOST_ROOT="$(env_value IRONCLAW_INSTANCE_HOST_ROOT)"
USER_ID="$(env_value IRONCLAW_REBORN_WEBUI_USER_ID)"
BIND_PORT="$(env_value IRONCLAW_BIND_PORT)"
TENANT_ID="${IRONCLAW_BUDGET_TENANT_ID:-reborn-cli}"

[[ -n "$HOST_ROOT" ]] || HOST_ROOT="/var/lib/ironclaw/instances/${INSTANCE_ID}"
[[ -n "$USER_ID" ]] || USER_ID="${INSTANCE_ID}-web"
[[ -n "$BIND_PORT" ]] || { echo "ERROR: missing IRONCLAW_BIND_PORT in instance environment" >&2; exit 1; }

INSTANCE_DB_DIR="/proc/${MAIN_PID}/root/srv/ironclaw-instance/home/local-dev"
DATABASE="reborn-local-dev.db"
DATABASE_DISPLAY="${HOST_ROOT}/home/local-dev/${DATABASE}"
cd "$INSTANCE_DB_DIR" || {
  echo "ERROR: cannot enter instance database directory through $INSTANCE_DB_DIR" >&2
  exit 1
}
[[ -f "$DATABASE" ]] || { echo "ERROR: budget database not found: $DATABASE_DISPLAY" >&2; exit 1; }

budget_command() {
  "$BINARY" budget "$@" --database "$DATABASE" --tenant "$TENANT_ID" --user "$USER_ID"
}

case "$ACTION" in
  status)
    COPY="$(mktemp --suffix=.db)"
    cleanup_status() {
      rm -f "$COPY" "${COPY}-wal" "${COPY}-shm"
    }
    trap cleanup_status EXIT
    sqlite3 "$DATABASE" ".timeout 5000" ".backup '$COPY'"
    "$BINARY" budget status --database "$COPY" --tenant "$TENANT_ID" --user "$USER_ID"
    ;;

  reset-period)
    [[ "$CONFIRM" == "--confirm-reset" ]] || {
      echo "ERROR: reset-period requires --confirm-reset" >&2
      exit 2
    }
    systemctl is-active --quiet "$UNIT" || {
      echo "ERROR: $UNIT must be active before the managed reset" >&2
      exit 1
    }

    TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
    BACKUP="${DATABASE}.bak-budget-reset-${TIMESTAMP}"
    BACKUP_DISPLAY="${DATABASE_DISPLAY}.bak-budget-reset-${TIMESTAMP}"
    OWNER="$(stat -c '%u:%g' "$DATABASE")"
    MODE="$(stat -c '%a' "$DATABASE")"

    STOPPED=0
    BACKUP_READY=0
    MUTATED=0
    rollback() {
      local original_rc="$1"
      trap - EXIT
      set +e
      if [[ "$MUTATED" -eq 1 && "$BACKUP_READY" -eq 1 ]]; then
        echo "ERROR: reset failed; restoring $BACKUP" >&2
        systemctl stop "$UNIT"
        cp -- "$BACKUP" "$DATABASE"
        chown "$OWNER" "$DATABASE"
        chmod "$MODE" "$DATABASE"
        rm -f "${DATABASE}-wal" "${DATABASE}-shm"
      else
        echo "ERROR: reset failed before database mutation" >&2
      fi
      if [[ "$STOPPED" -eq 1 || "$MUTATED" -eq 1 ]]; then
        systemctl start "$UNIT"
      fi
      exit "$original_rc"
    }
    on_exit() {
      local rc="$?"
      if [[ "$rc" -ne 0 ]]; then
        rollback "$rc"
      fi
    }
    trap on_exit EXIT

    systemctl stop "$UNIT"
    STOPPED=1
    systemctl is-active --quiet "$UNIT" && {
      echo "ERROR: $UNIT did not stop" >&2
      exit 1
    }

    sqlite3 "$DATABASE" ".timeout 5000" ".backup '$BACKUP'"
    [[ "$(sqlite3 "$BACKUP" 'PRAGMA quick_check;')" == "ok" ]] || {
      echo "ERROR: backup integrity check failed: $BACKUP" >&2
      exit 1
    }
    BACKUP_READY=1
    echo "Backup: $BACKUP_DISPLAY"

    MUTATED=1
    budget_command reset-period --confirm-reset

    chown "$OWNER" "$DATABASE"
    chmod "$MODE" "$DATABASE"
    for ledger_file in "${DATABASE}-wal" "${DATABASE}-shm"; do
      if [[ -e "$ledger_file" ]]; then
        chown "$OWNER" "$ledger_file"
        chmod 600 "$ledger_file"
      fi
    done

    systemctl start "$UNIT"
    STOPPED=0
    HEALTH_URL="http://127.0.0.1:${BIND_PORT}/api/health"
    HEALTHY=0
    for _ in $(seq 1 45); do
      if systemctl is-active --quiet "$UNIT" && curl -fsS --max-time 3 "$HEALTH_URL" >/dev/null; then
        HEALTHY=1
        break
      fi
      sleep 1
    done
    [[ "$HEALTHY" -eq 1 ]] || {
      echo "ERROR: $UNIT failed its health check after reset" >&2
      exit 1
    }

    sleep 10
    systemctl is-active --quiet "$UNIT"
    curl -fsS --max-time 3 "$HEALTH_URL" >/dev/null

    MUTATED=0
    trap - EXIT
    echo "Reset complete: $UNIT is stable and healthy"
    echo "Rollback backup retained: $BACKUP_DISPLAY"
    ;;

  *)
    usage >&2
    exit 2
    ;;
esac
