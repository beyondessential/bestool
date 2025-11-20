use std::{
	collections::HashMap, io::Write, ops::ControlFlow, path::PathBuf, process::Stdio, sync::Arc,
	time::Duration,
};

use chrono::{DateTime, Utc};
use miette::{Context as _, IntoDiagnostic, Result, miette};
use tera::Context as TeraCtx;
use tokio::io::AsyncReadExt as _;
use tokio_postgres::types::ToSql;
use tracing::{debug, error, info, instrument, warn};

use crate::{
	EmailConfig,
	pg_interval::Interval,
	targets::{ExternalTarget, SendTarget},
	templates::build_context,
};

fn enabled() -> bool {
	true
}

#[derive(serde::Deserialize, Debug, Default, Clone)]
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

#[derive(serde::Deserialize, Debug, Default, Clone)]
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
		&self,
		ctx: Arc<InternalContext>,
		email: Option<&EmailConfig>,
		dry_run: bool,
	) -> Result<()> {
		info!(?self.file, "executing alert");

		let now = chrono::Utc::now();
		let not_before = now - self.interval;
		info!(?now, ?not_before, interval=?self.interval, "date range for alert");

		let mut tera_ctx = build_context(self, now);
		if self
			.read_sources(&ctx.pg_client, not_before, &mut tera_ctx)
			.await?
			.is_break()
		{
			return Ok(());
		}

		for target in &self.send {
			if let Err(err) = target
				.send(self, ctx.clone(), &mut tera_ctx, email, dry_run)
				.await
			{
				error!("sending: {err:?}");
			}
		}

		Ok(())
	}
}

pub struct InternalContext {
	pub pg_client: tokio_postgres::Client,
	pub http_client: reqwest::Client,
}

fn rows_to_value_map(
	rows: &[tokio_postgres::Row],
) -> Vec<serde_json::Map<String, serde_json::Value>> {
	rows.iter()
		.map(|row| {
			let mut map = serde_json::Map::new();
			for (idx, column) in row.columns().iter().enumerate() {
				let value = postgres_to_value(row, idx);
				map.insert(column.name().to_string(), value);
			}
			map
		})
		.collect()
}

fn postgres_to_value(row: &tokio_postgres::Row, idx: usize) -> serde_json::Value {
	use tokio_postgres::types::Type;

	let column = &row.columns()[idx];
	match column.type_() {
		&Type::BOOL => row
			.get::<_, Option<bool>>(idx)
			.map(serde_json::Value::Bool)
			.unwrap_or(serde_json::Value::Null),
		&Type::INT2 => row
			.get::<_, Option<i16>>(idx)
			.map(|v| serde_json::Value::Number(v.into()))
			.unwrap_or(serde_json::Value::Null),
		&Type::INT4 => row
			.get::<_, Option<i32>>(idx)
			.map(|v| serde_json::Value::Number(v.into()))
			.unwrap_or(serde_json::Value::Null),
		&Type::INT8 => row
			.get::<_, Option<i64>>(idx)
			.map(|v| serde_json::Value::Number(v.into()))
			.unwrap_or(serde_json::Value::Null),
		&Type::FLOAT4 => row
			.get::<_, Option<f32>>(idx)
			.and_then(|v| serde_json::Number::from_f64(v as f64))
			.map(serde_json::Value::Number)
			.unwrap_or(serde_json::Value::Null),
		&Type::FLOAT8 => row
			.get::<_, Option<f64>>(idx)
			.and_then(serde_json::Number::from_f64)
			.map(serde_json::Value::Number)
			.unwrap_or(serde_json::Value::Null),
		&Type::TEXT | &Type::VARCHAR => row
			.get::<_, Option<String>>(idx)
			.map(serde_json::Value::String)
			.unwrap_or(serde_json::Value::Null),
		&Type::JSON | &Type::JSONB => {
			let val: Option<::serde_json::Value> = row.get(idx);
			val.unwrap_or(::serde_json::Value::Null)
		}
		&Type::TIMESTAMP | &Type::TIMESTAMPTZ => row
			.get::<_, Option<chrono::NaiveDateTime>>(idx)
			.map(|dt| serde_json::Value::String(dt.to_string()))
			.unwrap_or(serde_json::Value::Null),
		_ => row
			.get::<_, Option<String>>(idx)
			.map(serde_json::Value::String)
			.unwrap_or(serde_json::Value::Null),
	}
}
