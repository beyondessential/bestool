use std::fmt::Display;

use miette::{Context as _, IntoDiagnostic, Result};
use sysinfo::System;
use tera::{Context as TeraCtx, Tera};
use tracing::instrument;

use crate::alert::AlertDefinition;

const DEFAULT_SUBJECT_TEMPLATE: &str = "[Tamanu Alert] {{ filename }} ({{ hostname }})";

#[derive(serde::Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TemplateField {
	Filename,
	Subject,
	Body,
	Hostname,
	Interval,
}

impl TemplateField {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Filename => "filename",
			Self::Subject => "subject",
			Self::Body => "body",
			Self::Hostname => "hostname",
			Self::Interval => "interval",
		}
	}
}

impl Display for TemplateField {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.as_str())
	}
}

#[instrument]
pub fn load_templates(subject: &Option<String>, template: &str) -> Result<Tera> {
	let mut tera = tera::Tera::default();

	tera.add_raw_template(
		TemplateField::Subject.as_str(),
		subject.as_deref().unwrap_or(DEFAULT_SUBJECT_TEMPLATE),
	)
	.into_diagnostic()
	.wrap_err("compiling subject template")?;
	tera.add_raw_template(TemplateField::Body.as_str(), template)
		.into_diagnostic()
		.wrap_err("compiling body template")?;

	Ok(tera)
}

#[instrument(skip(alert, now))]
pub fn build_context(alert: &AlertDefinition, now: chrono::DateTime<chrono::Utc>) -> TeraCtx {
	let mut context = TeraCtx::new();
	context.insert(
		TemplateField::Interval.as_str(),
		&jiff::Span::try_from(alert.interval_duration)
			.ok()
			.map(|s| format!("{s:#}"))
			.unwrap_or_else(|| format!("{:?}", alert.interval_duration)),
	);
	context.insert(
		TemplateField::Hostname.as_str(),
		System::host_name().as_deref().unwrap_or("unknown"),
	);
	context.insert(
		TemplateField::Filename.as_str(),
		&alert.file.file_name().unwrap().to_string_lossy(),
	);
	context.insert("now", &now.to_string());

	context
}

#[instrument(skip(tera, context))]
pub fn render_alert(tera: &Tera, context: &mut TeraCtx) -> Result<(String, String)> {
	let subject = tera
		.render(TemplateField::Subject.as_str(), context)
		.into_diagnostic()
		.wrap_err("rendering subject template")?;

	context.insert(TemplateField::Subject.as_str(), &subject.to_string());

	let body = tera
		.render(TemplateField::Body.as_str(), context)
		.into_diagnostic()
		.wrap_err("rendering email template")?;

	Ok((subject, body))
}
