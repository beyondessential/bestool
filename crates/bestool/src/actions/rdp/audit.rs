use std::{net::IpAddr, path::Path};

use chrono::{DateTime, Utc};
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::Serialize;
use tokio::{
	fs::{File, OpenOptions, create_dir_all},
	io::AsyncWriteExt,
};

use super::events::Event;

/// Append-only JSONL audit log of every RDP session event.
///
/// The file is opened in append mode so multiple writers would coexist safely,
/// though only one `bestool rdp monitor` should run per host.
pub struct AuditLog {
	file: File,
}

impl AuditLog {
	pub async fn open(path: &Path) -> Result<Self> {
		if let Some(parent) = path.parent()
			&& !parent.as_os_str().is_empty()
		{
			create_dir_all(parent)
				.await
				.into_diagnostic()
				.wrap_err_with(|| format!("creating {}", parent.display()))?;
		}

		let file = OpenOptions::new()
			.create(true)
			.append(true)
			.open(path)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("opening {}", path.display()))?;
		Ok(Self { file })
	}

	pub async fn append(&mut self, ev: &Event, tailscale_user: Option<&str>) -> Result<()> {
		let record = Record {
			time: ev.time,
			event: ev.kind.as_str(),
			session: ev.session_id,
			user: &ev.user,
			address: ev.address,
			tailscale_user,
		};
		let mut line = serde_json::to_vec(&record).into_diagnostic()?;
		line.push(b'\n');
		self.file.write_all(&line).await.into_diagnostic()?;
		self.file.flush().await.into_diagnostic()?;
		Ok(())
	}
}

#[derive(Debug, Serialize)]
struct Record<'a> {
	#[serde(rename = "ts")]
	time: DateTime<Utc>,
	event: &'a str,
	session: u32,
	user: &'a str,
	#[serde(skip_serializing_if = "Option::is_none")]
	address: Option<IpAddr>,
	#[serde(skip_serializing_if = "Option::is_none", rename = "tailscale")]
	tailscale_user: Option<&'a str>,
}

#[cfg(test)]
mod tests {
	use std::net::Ipv4Addr;

	use super::{super::events::EventKind, *};

	#[tokio::test]
	async fn writes_jsonl_lines() {
		let tmp = tempfile::NamedTempFile::new().unwrap();
		let path = tmp.path().to_path_buf();
		drop(tmp);

		let mut log = AuditLog::open(&path).await.unwrap();
		log.append(
			&Event {
				kind: EventKind::Logon,
				session_id: 2,
				user: r"CORP\alice".into(),
				address: Some(IpAddr::V4(Ipv4Addr::new(100, 64, 1, 5))),
				time: "2026-04-22T10:00:00Z".parse().unwrap(),
				record_id: 1,
			},
			Some("alice@bes.au"),
		)
		.await
		.unwrap();
		log.append(
			&Event {
				kind: EventKind::Logoff,
				session_id: 2,
				user: r"CORP\alice".into(),
				address: None,
				time: "2026-04-22T10:10:00Z".parse().unwrap(),
				record_id: 2,
			},
			None,
		)
		.await
		.unwrap();

		let contents = tokio::fs::read_to_string(&path).await.unwrap();
		let lines: Vec<_> = contents.lines().collect();
		assert_eq!(lines.len(), 2);

		let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
		assert_eq!(first["event"], "logon");
		assert_eq!(first["session"], 2);
		assert_eq!(first["address"], "100.64.1.5");
		assert_eq!(first["tailscale"], "alice@bes.au");

		let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
		assert_eq!(second["event"], "logoff");
		assert!(second.get("address").is_none());
		assert!(second.get("tailscale").is_none());

		tokio::fs::remove_file(&path).await.ok();
	}
}
