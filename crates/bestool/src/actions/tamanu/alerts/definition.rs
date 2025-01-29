use std::{
	collections::HashMap, io::Write, ops::ControlFlow, path::PathBuf, process::Stdio, sync::Arc,
	time::Duration,
};

use chrono::{DateTime, Utc};
use miette::{miette, Context as _, IntoDiagnostic, Result};
use tera::Context as TeraCtx;
use tokio::io::AsyncReadExt as _;
use tokio_postgres::types::ToSql;
use tracing::{debug, error, info, instrument, warn};

use crate::{actions::tamanu::config::TamanuConfig, postgres_to_value::rows_to_value_map};

use super::{
	pg_interval::Interval,
	targets::{ExternalTarget, SendTarget},
	templates::build_context,
	InternalContext,
};

fn enabled() -> bool {
	true
}

#[derive(serde::Deserialize, Debug, Default)]
pub struct AlertDefinition {
	#[serde(default, skip)]
	pub file: PathBuf,

	#[serde(default = "enabled")]
	pub enabled: bool,
	#[serde(skip)]
	pub interval: Duration,
	#[serde(default)]
	pub send: Vec<SendTarget>,

	#[serde(flatten)]
	pub source: TicketSource,
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(untagged, deny_unknown_fields)]
pub enum TicketSource {
	Sql {
		sql: String,
	},
	Shell {
		shell: String,
		run: String,
	},

	#[default]
	None,
}

impl AlertDefinition {
	pub fn normalise(mut self, external_targets: &HashMap<String, Vec<ExternalTarget>>) -> Self {
		self.send = self
			.send
			.iter()
			.flat_map(|target| match target {
				target @ SendTarget::External { id, .. } => target
					.resolve_external(external_targets)
					.unwrap_or_else(|| {
						error!(id, "external target not found");
						Vec::new()
					}),
				other => vec![other.clone()],
			})
			.collect();

		self
	}

	#[instrument(skip(self, client, not_before, context))]
	pub async fn read_sources(
		&self,
		client: &tokio_postgres::Client,
		not_before: DateTime<Utc>,
		context: &mut TeraCtx,
	) -> Result<ControlFlow<(), ()>> {
		match &self.source {
			TicketSource::None => {
				debug!(?self.file, "no source, skipping");
				return Ok(ControlFlow::Break(()));
			}
			TicketSource::Sql { sql } => {
				let statement = client.prepare(sql).await.into_diagnostic()?;

				let interval = Interval(self.interval);
				let all_params: Vec<&(dyn ToSql + Sync)> = vec![&not_before, &interval];

				let rows = client
					.query(&statement, &all_params[..statement.params().len()])
					.await
					.into_diagnostic()
					.wrap_err("querying database")?;

				if rows.is_empty() {
					debug!(?self.file, "no rows returned, skipping");
					return Ok(ControlFlow::Break(()));
				}
				info!(?self.file, rows=%rows.len(), "alert triggered");

				let context_rows = rows_to_value_map(&rows);

				context.insert("rows", &context_rows);
			}
			TicketSource::Shell { shell, run } => {
				let mut script = tempfile::Builder::new().tempfile().into_diagnostic()?;
				write!(script.as_file_mut(), "{run}").into_diagnostic()?;

				let mut shell = tokio::process::Command::new(shell)
					.arg(script.path())
					.stdin(Stdio::null())
					.stdout(Stdio::piped())
					.spawn()
					.into_diagnostic()?;

				let mut output = Vec::new();
				let mut stdout = shell
					.stdout
					.take()
					.ok_or_else(|| miette!("getting the child stdout handle"))?;
				let output_future =
					futures::future::try_join(shell.wait(), stdout.read_to_end(&mut output));

				let Ok(res) = tokio::time::timeout(self.interval, output_future).await else {
					warn!(?self.file, "the script timed out, skipping");
					shell.kill().await.into_diagnostic()?;
					return Ok(ControlFlow::Break(()));
				};

				let (status, output_size) = res.into_diagnostic().wrap_err("running the shell")?;

				if status.success() {
					debug!(?self.file, "the script succeeded, skipping");
					return Ok(ControlFlow::Break(()));
				}
				info!(?self.file, ?status, ?output_size, "alert triggered");

				context.insert("output", &String::from_utf8_lossy(&output));
			}
		}
		Ok(ControlFlow::Continue(()))
	}

	pub async fn execute(
		self,
		ctx: Arc<InternalContext>,
		config: &TamanuConfig,
		dry_run: bool,
	) -> Result<()> {
		info!(?self.file, "executing alert");

		let now = crate::now_time(&chrono::Utc);
		let not_before = now - self.interval;
		info!(?now, ?not_before, interval=?self.interval, "date range for alert");

		let mut tera_ctx = build_context(&self, now);
		if self
			.read_sources(&ctx.pg_client, not_before, &mut tera_ctx)
			.await?
			.is_break()
		{
			return Ok(());
		}

		for target in &self.send {
			if let Err(err) = target
				.send(&self, ctx.clone(), &mut tera_ctx, config, dry_run)
				.await
			{
				error!("sending: {err:?}");
			}
		}

		Ok(())
	}
}
