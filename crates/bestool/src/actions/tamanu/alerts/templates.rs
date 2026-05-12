use std::{fmt::Display, time::Duration};

use miette::{Context as _, IntoDiagnostic, Result};
use sysinfo::System;
use tera::{Context as TeraCtx, Tera};
use tracing::{instrument, warn};

use super::{
	definition::AlertDefinition,
	targets::{
		SendTarget,
		zendesk::{TargetZendesk, ZendeskMethod},
	},
};

const DEFAULT_SUBJECT_TEMPLATE: &str = "[Tamanu Alert] {{ filename }} ({{ hostname }})";

#[derive(serde::Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TemplateField {
	Filename,
	Subject,
	Body,
	Hostname,
	Requester,
	Interval,
}

impl TemplateField {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Filename => "filename",
			Self::Subject => "subject",
			Self::Body => "body",
			Self::Hostname => "hostname",
			Self::Requester => "requester",
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
pub fn load_templates(target: &SendTarget) -> Result<Tera> {
	let mut tera = tera::Tera::default();

	match target {
		SendTarget::Email {
			subject, template, ..
		}
		| SendTarget::Zendesk {
			subject, template, ..
		}
		| SendTarget::Slack {
			subject, template, ..
		}
		| SendTarget::Canopy {
			subject, template, ..
		}
		| SendTarget::External {
			subject, template, ..
		} => {
			tera.add_raw_template(
				TemplateField::Subject.as_str(),
				subject.as_deref().unwrap_or(DEFAULT_SUBJECT_TEMPLATE),
			)
			.into_diagnostic()
			.wrap_err("compiling subject template")?;
			tera.add_raw_template(TemplateField::Body.as_str(), template)
				.into_diagnostic()
				.wrap_err("compiling body template")?;
		}
	}

	if let SendTarget::Zendesk {
		conn: TargetZendesk {
			method: ZendeskMethod::Anonymous { requester },
			..
		},
		..
	} = target
	{
		tera.add_raw_template(TemplateField::Requester.as_str(), requester)
			.into_diagnostic()
			.wrap_err("compiling requester template")?;
	}
	Ok(tera)
}

/// Format a duration as a single human-friendly unit, dropping any remainder.
///
/// E.g. 90 minutes prints as "1h"; 1 day as "1d"; 30 seconds as "30s".
pub(crate) fn humanize_duration(dur: Duration) -> String {
	let secs = dur.as_secs();
	if secs >= 86400 {
		format!("{}d", secs / 86400)
	} else if secs >= 3600 {
		format!("{}h", secs / 3600)
	} else if secs >= 60 {
		format!("{}m", secs / 60)
	} else {
		format!("{}s", secs)
	}
}

#[instrument(skip(alert, now))]
pub fn build_context(alert: &AlertDefinition, now: jiff::Timestamp) -> TeraCtx {
	let mut context = TeraCtx::new();
	context.insert(
		TemplateField::Interval.as_str(),
		&humanize_duration(alert.interval),
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
pub fn render_alert(
	tera: &Tera,
	context: &mut TeraCtx,
) -> Result<(String, String, Option<String>)> {
	let subject = tera
		.render(TemplateField::Subject.as_str(), context)
		.into_diagnostic()
		.wrap_err("rendering subject template")?;

	context.insert(TemplateField::Subject.as_str(), &subject.to_string());

	let body = tera
		.render(TemplateField::Body.as_str(), context)
		.into_diagnostic()
		.wrap_err("rendering email template")?;

	let requester = tera
		.render(TemplateField::Requester.as_str(), context)
		.map(Some)
		.or_else(|err| match err.kind {
			tera::ErrorKind::TemplateNotFound(_) => Ok(None),
			_ => Err(err),
		})
		.into_diagnostic()
		.wrap_err("rendering requester template")?;

	Ok((subject, body, requester))
}
