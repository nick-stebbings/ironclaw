use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use ironclaw_reborn_composition::{
    LibSqlBudgetAdmin, RebornBudgetAccountStatus, RebornBudgetAdminError,
};

#[derive(Debug, Args)]
pub(crate) struct BudgetCommand {
    #[command(subcommand)]
    command: BudgetSubcommand,
}

#[derive(Debug, Subcommand)]
enum BudgetSubcommand {
    /// Read one user budget account from an offline database or coherent backup.
    Status(BudgetStatusCommand),
    /// Clear one user's accumulated period usage while preserving limits and reservations.
    ResetPeriod(BudgetResetPeriodCommand),
}

#[derive(Debug, Args)]
struct BudgetStatusCommand {
    #[command(flatten)]
    target: BudgetTarget,
}

#[derive(Debug, Args)]
struct BudgetResetPeriodCommand {
    #[command(flatten)]
    target: BudgetTarget,
    /// Confirm that the owning runtime is stopped and this reset is intentional.
    #[arg(long)]
    confirm_reset: bool,
}

#[derive(Debug, Args)]
struct BudgetTarget {
    /// Existing libSQL database. The owning runtime must be stopped.
    #[arg(long, value_name = "PATH")]
    database: PathBuf,
    /// Tenant that owns the user budget account.
    #[arg(long, default_value = "reborn-cli")]
    tenant: String,
    /// User budget account to inspect or reset.
    #[arg(long)]
    user: String,
}

impl BudgetCommand {
    pub(crate) fn execute(self) -> anyhow::Result<()> {
        match self.command {
            BudgetSubcommand::Status(command) => execute_status(command),
            BudgetSubcommand::ResetPeriod(command) => execute_reset_period(command),
        }
    }
}

fn execute_status(command: BudgetStatusCommand) -> anyhow::Result<()> {
    let status = with_admin(&command.target, |admin| {
        admin.status(&command.target.tenant, &command.target.user)
    })?;
    print_status(&status)
}

fn execute_reset_period(command: BudgetResetPeriodCommand) -> anyhow::Result<()> {
    if !command.confirm_reset {
        bail!("reset-period requires --confirm-reset");
    }
    let status = with_admin(&command.target, |admin| {
        admin.reset_period(&command.target.tenant, &command.target.user)
    })?;
    print_status(&status)
}

fn with_admin<T>(
    target: &BudgetTarget,
    operation: impl FnOnce(&LibSqlBudgetAdmin) -> Result<T, RebornBudgetAdminError>,
) -> anyhow::Result<T> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build budget admin runtime")?;
    let admin = runtime
        .block_on(LibSqlBudgetAdmin::open(&target.database))
        .with_context(|| format!("open budget database {}", target.database.display()))?;
    operation(&admin).context("budget operation")
}

fn print_status(status: &RebornBudgetAccountStatus) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(status).context("serialize budget status")?
    );
    Ok(())
}
