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
[[ -n "$BIND_PORT" ]] || {
  echo "ERROR: missing IRONCLAW_BIND_PORT in instance environment" >&2
  exit 1
}

INSTANCE_DB_DIR="/proc/${MAIN_PID}/root/srv/ironclaw-instance/home/local-dev"
DATABASE="reborn-local-dev.db"
DATABASE_DISPLAY="${HOST_ROOT}/home/local-dev/${DATABASE}"
cd "$INSTANCE_DB_DIR" || {
  echo "ERROR: cannot hold instance database directory: $INSTANCE_DB_DIR" >&2
  exit 1
}
[[ -f "$DATABASE" ]] || {
  echo "ERROR: budget database not found: $DATABASE_DISPLAY" >&2
  exit 1
}

WORK_DIR=""
cleanup_work_dir() {
  if [[ -n "$WORK_DIR" && -d "$WORK_DIR" ]]; then
    rm -rf -- "$WORK_DIR"
  fi
}

stage_database_copy() {
  local destination="$1"
  cp -- "$DATABASE" "$destination/$DATABASE"
  if [[ -e "${DATABASE}-wal" ]]; then
    cp -- "${DATABASE}-wal" "$destination/${DATABASE}-wal"
  fi
}

budget_command() {
  local database="$1"
  shift
  "$BINARY" budget "$@" --database "$database" --tenant "$TENANT_ID" --user "$USER_ID"
}

case "$ACTION" in
  status)
    WORK_DIR="$(mktemp -d)"
    FROZEN=0
    cleanup_status() {
      if [[ "$FROZEN" -eq 1 ]]; then
        kill -CONT "$MAIN_PID" 2>/dev/null || true
      fi
      cleanup_work_dir
    }
    trap cleanup_status EXIT
    kill -STOP "$MAIN_PID"
    FROZEN=1
    stage_database_copy "$WORK_DIR"
    kill -CONT "$MAIN_PID"
    FROZEN=0
    SNAPSHOT="$WORK_DIR/snapshot.db"
    sqlite3 "$WORK_DIR/$DATABASE" ".timeout 5000" ".backup '$SNAPSHOT'"
    budget_command "$SNAPSHOT" status
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
    OWNER_UID="$(stat -c '%u' "$DATABASE")"
    OWNER_GROUP="$(stat -c '%g' "$DATABASE")"
    MODE="$(stat -c '%a' "$DATABASE")"
    if [[ "$EUID" -ne 0 && "$OWNER_UID" -ne "$EUID" ]]; then
      echo "ERROR: database is owned by uid $OWNER_UID; run as that user or root" >&2
      exit 1
    fi

    STOPPED=0
    BACKUP_READY=0
    MUTATED=0

    restore_metadata() {
      chmod "$MODE" "$DATABASE"
      if [[ "$EUID" -eq 0 ]]; then
        chown "${OWNER_UID}:${OWNER_GROUP}" "$DATABASE"
      fi
    }

    rollback() {
      local original_rc="$1"
      trap - EXIT
      set +e
      if [[ "$MUTATED" -eq 1 && "$BACKUP_READY" -eq 1 ]]; then
        echo "ERROR: reset failed; restoring $BACKUP_DISPLAY" >&2
        systemctl stop "$UNIT"
        cp -- "$BACKUP" "$DATABASE"
        rm -f "${DATABASE}-wal" "${DATABASE}-shm"
        restore_metadata
      else
        echo "ERROR: reset failed before database mutation" >&2
      fi
      if [[ "$STOPPED" -eq 1 || "$MUTATED" -eq 1 ]]; then
        systemctl start "$UNIT"
      fi
      cleanup_work_dir
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

    WORK_DIR="$(mktemp -d)"
    stage_database_copy "$WORK_DIR"
    BACKUP_STAGE="$WORK_DIR/pre-reset.db"
    sqlite3 "$WORK_DIR/$DATABASE" ".timeout 5000" ".backup '$BACKUP_STAGE'"
    [[ "$(sqlite3 "$BACKUP_STAGE" 'PRAGMA quick_check;')" == "ok" ]] || {
      echo "ERROR: backup integrity check failed" >&2
      exit 1
    }
    cp -- "$BACKUP_STAGE" "$BACKUP"
    chmod "$MODE" "$BACKUP"
    BACKUP_READY=1
    echo "Backup: $BACKUP_DISPLAY"

    CANDIDATE="$WORK_DIR/reset.db"
    INSTALL_STAGE="$WORK_DIR/install.db"
    cp -- "$BACKUP_STAGE" "$CANDIDATE"
    budget_command "$CANDIDATE" reset-period --confirm-reset
    sqlite3 "file:${CANDIDATE}?mode=ro" ".backup '$INSTALL_STAGE'"
    [[ "$(sqlite3 "$INSTALL_STAGE" 'PRAGMA quick_check;')" == "ok" ]] || {
      echo "ERROR: reset database integrity check failed" >&2
      exit 1
    }
    budget_command "$INSTALL_STAGE" status >/dev/null

    MUTATED=1
    cp -- "$INSTALL_STAGE" "$DATABASE"
    rm -f "${DATABASE}-wal" "${DATABASE}-shm"
    restore_metadata

    systemctl start "$UNIT"
    STOPPED=0
    HEALTH_URL="http://127.0.0.1:${BIND_PORT}/api/health"
    HEALTHY=0
    for _ in $(seq 1 45); do
      if systemctl is-active --quiet "$UNIT" && curl -fsS --max-time 3 "$HEALTH_URL" >/dev/null 2>&1; then
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
    cleanup_work_dir
    echo "Reset complete: $UNIT is stable and healthy"
    echo "Rollback backup retained: $BACKUP_DISPLAY"
    ;;

  *)
    usage >&2
    exit 2
    ;;
esac
