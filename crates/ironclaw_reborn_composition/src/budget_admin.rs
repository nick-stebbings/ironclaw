//! Offline operator facade for durable budget-account state.

use std::path::Path;
use std::sync::Arc;

use ironclaw_filesystem::{LibSqlRootFilesystem, ScopedFilesystem};
use ironclaw_host_api::{
    MountAlias, MountGrant, MountPermissions, MountView, TenantId, UserId, VirtualPath,
};
use ironclaw_resources::{
    AccountSnapshot, FilesystemResourceGovernor, ResourceAccount, ResourceGovernor,
};
use serde::Serialize;
use thiserror::Error;

const SYSTEM_RESOURCES_PATH: &str = "/tenants/__system__/users/__system__/resources";

#[derive(Debug, Error)]
pub enum RebornBudgetAdminError {
    #[error("invalid {field}: {reason}")]
    InvalidIdentity { field: &'static str, reason: String },
    #[error("failed to open budget database: {reason}")]
    Database { reason: String },
    #[error("failed to configure budget storage: {reason}")]
    Storage { reason: String },
    #[error("budget account not found: {account}")]
    AccountNotFound { account: String },
    #[error("budget governor operation failed: {reason}")]
    Governor { reason: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct RebornBudgetAccountStatus {
    pub account: String,
    pub limits: serde_json::Value,
    pub period_start: String,
    pub period_end: String,
    pub spent: serde_json::Value,
    pub reserved: serde_json::Value,
}

impl RebornBudgetAccountStatus {
    fn from_snapshot(snapshot: AccountSnapshot) -> Result<Self, RebornBudgetAdminError> {
        let limits = serde_json::to_value(&snapshot.limits).map_err(|error| {
            RebornBudgetAdminError::Storage {
                reason: error.to_string(),
            }
        })?;
        let spent = serde_json::to_value(&snapshot.ledger.spent).map_err(|error| {
            RebornBudgetAdminError::Storage {
                reason: error.to_string(),
            }
        })?;
        let reserved = serde_json::to_value(&snapshot.ledger.reserved).map_err(|error| {
            RebornBudgetAdminError::Storage {
                reason: error.to_string(),
            }
        })?;
        Ok(Self {
            account: snapshot.account.to_string(),
            limits,
            period_start: snapshot.ledger.period_start.to_rfc3339(),
            period_end: snapshot.ledger.period_end.to_rfc3339(),
            spent,
            reserved,
        })
    }
}

pub struct LibSqlBudgetAdmin {
    governor: FilesystemResourceGovernor<LibSqlRootFilesystem>,
}

impl LibSqlBudgetAdmin {
    /// Open an existing libSQL budget ledger.
    ///
    /// The runtime owning this database must be stopped, or the caller must
    /// operate on a coherent SQLite backup. Filesystem governors are
    /// process-local singleton authorities.
    pub async fn open(database_path: &Path) -> Result<Self, RebornBudgetAdminError> {
        let database = Arc::new(
            libsql::Builder::new_local(database_path)
                .build()
                .await
                .map_err(|error| RebornBudgetAdminError::Database {
                    reason: error.to_string(),
                })?,
        );
        let filesystem = Arc::new(LibSqlRootFilesystem::new(database));
        let mounts = MountView::new(vec![MountGrant::new(
            MountAlias::new("/resources").map_err(|reason| RebornBudgetAdminError::Storage {
                reason: reason.to_string(),
            })?,
            VirtualPath::new(SYSTEM_RESOURCES_PATH).map_err(|reason| {
                RebornBudgetAdminError::Storage {
                    reason: reason.to_string(),
                }
            })?,
            MountPermissions::read_write_list_delete(),
        )])
        .map_err(|reason| RebornBudgetAdminError::Storage {
            reason: reason.to_string(),
        })?;
        let scoped = Arc::new(ScopedFilesystem::with_fixed_view(filesystem, mounts));
        let governor = FilesystemResourceGovernor::new(scoped);
        governor
            .warm_authority()
            .map_err(|error| RebornBudgetAdminError::Governor {
                reason: error.to_string(),
            })?;
        Ok(Self { governor })
    }

    pub fn status(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<RebornBudgetAccountStatus, RebornBudgetAdminError> {
        let account = user_account(tenant_id, user_id)?;
        let snapshot = self
            .governor
            .account_snapshot(&account)
            .map_err(|error| RebornBudgetAdminError::Governor {
                reason: error.to_string(),
            })?
            .ok_or_else(|| RebornBudgetAdminError::AccountNotFound {
                account: account.to_string(),
            })?;
        RebornBudgetAccountStatus::from_snapshot(snapshot)
    }

    pub fn reset_period(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<RebornBudgetAccountStatus, RebornBudgetAdminError> {
        let account = user_account(tenant_id, user_id)?;
        let snapshot = self
            .governor
            .reset_period(account.clone())
            .map_err(|error| RebornBudgetAdminError::Governor {
                reason: error.to_string(),
            })?
            .ok_or_else(|| RebornBudgetAdminError::AccountNotFound {
                account: account.to_string(),
            })?;
        RebornBudgetAccountStatus::from_snapshot(snapshot)
    }
}

fn user_account(tenant_id: &str, user_id: &str) -> Result<ResourceAccount, RebornBudgetAdminError> {
    let tenant_id =
        TenantId::new(tenant_id).map_err(|reason| RebornBudgetAdminError::InvalidIdentity {
            field: "tenant id",
            reason: reason.to_string(),
        })?;
    let user_id =
        UserId::new(user_id).map_err(|reason| RebornBudgetAdminError::InvalidIdentity {
            field: "user id",
            reason: reason.to_string(),
        })?;
    Ok(ResourceAccount::user(tenant_id, user_id))
}
