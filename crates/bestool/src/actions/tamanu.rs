use std::path::PathBuf;

use clap::{Parser, Subcommand};
use miette::Result;
use node_semver::Version;

use crate::args::Args;

use super::Context;

use bestool_tamanu::find_tamanu as _find_tamanu;

#[cfg(feature = "tamanu-lifecycle")]
pub mod lifecycle;
#[cfg(feature = "tamanu-lifecycle")]
mod probe;

/// Interact with Tamanu.
///
/// Alias: t
#[derive(Debug, Clone, Parser)]
pub struct TamanuArgs {
	/// Tamanu root to operate in
	#[arg(long)]
	pub root: Option<PathBuf>,

	/// Tamanu subcommand
	#[command(subcommand)]
	pub action: Action,

	#[doc(hidden)]
	#[arg(long, hide = true)]
	pub(crate) use_colours: bool,
}

super::subcommands! {
	[TamanuArgs => |mut args: TamanuArgs, mut ctx: Context| -> Result<(Action, Context)> {
		let top: &Args = ctx.require();
		args.use_colours = top.logging.color.enabled();
		let action = args.action.clone();
		ctx.provide(args);
		Ok((action, ctx))
	}]

	#[cfg(feature = "tamanu-alerts")]
	alerts => Alerts(AlertsArgs),
	#[cfg(feature = "tamanu-alertd")]
	alertd => Alertd(AlertdArgs),
	#[cfg(feature = "tamanu-artifacts")]
	#[clap(alias = "art")]
	artifacts => Artifacts(ArtifactsArgs),
	#[cfg(feature = "tamanu-backup")]
	#[clap(alias = "b")]
	backup => Backup(BackupArgs),
	#[cfg(feature = "tamanu-backup-configs")]
	backup_configs => BackupConfigs(BackupConfigsArgs),
	#[cfg(feature = "tamanu-config")]
	#[clap(alias = "c")]
	config => Config(ConfigArgs),
	#[cfg(feature = "tamanu-url")]
	#[clap(aliases = ["db", "u", "url"])]
	db_url => DbUrl(DbUrlArgs),
	#[cfg(feature = "tamanu-doctor")]
	#[clap(alias = "doc")]
	doctor => Doctor(DoctorArgs),
	#[cfg(feature = "tamanu-download")]
	#[clap(aliases = ["d", "down"])]
	download => Download(DownloadArgs),
	#[cfg(feature = "tamanu-find")]
	find => Find(FindArgs),
	#[cfg(feature = "tamanu-greenmask")]
	greenmask_config => GreenmaskConfig(GreenmaskConfigArgs),
	#[cfg(feature = "tamanu-logs")]
	#[clap(alias = "l")]
	logs => Logs(LogsArgs),
	#[cfg(feature = "tamanu-meta-ticket")]
	meta_ticket => MetaTicket(MetaTicketArgs),
	#[cfg(feature = "tamanu-psql")]
	#[clap(aliases = ["p", "pg", "sql"])]
	psql => Psql(PsqlArgs),
	#[cfg(feature = "tamanu-sync")]
	sync => Sync(SyncArgs),
	#[cfg(feature = "tamanu-tags")]
	tags => Tags(TagsArgs),
	#[cfg(feature = "tamanu-lifecycle")]
	restart => Restart(RestartArgs),
	#[cfg(feature = "tamanu-lifecycle")]
	start => Start(StartArgs),
	#[cfg(feature = "tamanu-lifecycle")]
	status => Status(StatusArgs),
	#[cfg(feature = "tamanu-lifecycle")]
	stop => Stop(StopArgs)
}

pub fn find_tamanu(args: &TamanuArgs) -> Result<(Version, PathBuf)> {
	_find_tamanu(args.root.as_deref())
}
