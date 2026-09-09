//! The audit log's on-disk record shape and its framing.
//!
//! spec: AUD-STO

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::tailscale::TailscalePeer;

/// Current record format version.
pub const FORMAT_VERSION: u32 = 1;

/// RFC 7464 record separator, written before each record.
pub const SEPARATOR: u8 = 0x1E;

/// Line feed, written after each record.
pub const TERMINATOR: u8 = 0x0A;

/// One record of the audit log.
///
/// Serialises to a single line of JSON with the common fields first and the
/// kind-specific fields flattened after them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
	/// Format version, so readers need not guess at the record shape.
	pub v: u32,
	/// Position of this record within its session, assigned when the record is
	/// made rather than when it lands.
	pub seq: u64,
	pub ts: Timestamp,
	/// Hash of the previous record of this session, empty for the first.
	pub prev: String,
	#[serde(flatten)]
	pub kind: RecordKind,
}

impl Record {
	/// Serialise to the JSON text that is hashed and framed.
	pub fn to_json(&self) -> Result<String, serde_json::Error> {
		serde_json::to_string(self)
	}

	/// The session this record belongs to, where the record says so itself.
	///
	/// Only context records carry the session identity; every other kind is
	/// attributed by following [`Record::prev`] back to one that does.
	pub fn instance(&self) -> Option<Uuid> {
		match &self.kind {
			RecordKind::Context(context) => Some(context.instance),
			_ => None,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RecordKind {
	/// Session state that applies to every query record after it.
	Context(ContextRecord),
	/// A statement the session ran.
	Query(QueryRecord),
	/// Records that were made but never written.
	Gap(GapRecord),
	/// A clean session exit.
	End,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextRecord {
	pub sys_user: String,
	pub db_user: String,
	pub writemode: bool,
	/// The over-the-shoulder supervisor named when write mode was enabled.
	pub ots: Option<String>,
	pub tailscale: Vec<TailscalePeer>,
	pub instance: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryRecord {
	pub query: String,
	#[serde(flatten)]
	pub source: QuerySource,
}

/// Where a statement came from.
///
/// Serialises flat, so `source` is always a string and stays greppable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum QuerySource {
	/// Typed at the prompt.
	Typed,
	/// Run by a named snippet.
	Snippet { name: String },
	/// Run by an included file, named by the absolute path it was opened at.
	Include { path: String },
	/// Imported from a store that did not record where the statement came from.
	Unknown,
}

impl QuerySource {
	/// Whether a statement from this source is recalled as shell history.
	///
	/// spec: AUD-HIS
	pub fn is_recallable(&self) -> bool {
		matches!(self, Self::Typed)
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GapRecord {
	/// How many records were lost.
	pub lost: u64,
	/// The last sequence number the lost records held.
	pub through: u64,
	/// When the earliest lost record was made.
	pub from: Timestamp,
	/// When the latest lost record was made.
	pub to: Timestamp,
}

/// Hash a record's JSON text: the bytes between its separator and its newline.
pub fn hash(json: &str) -> String {
	let mut hasher = Sha256::new();
	hasher.update(json.as_bytes());
	hex::encode(hasher.finalize())
}

/// Wrap a record's JSON text in its framing bytes.
pub fn frame(json: &str) -> Vec<u8> {
	let mut out = Vec::with_capacity(json.len() + 2);
	out.push(SEPARATOR);
	out.extend_from_slice(json.as_bytes());
	out.push(TERMINATOR);
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn context() -> Record {
		Record {
			v: FORMAT_VERSION,
			seq: 0,
			ts: "2026-09-08T03:14:15.926535Z".parse().unwrap(),
			prev: String::new(),
			kind: RecordKind::Context(ContextRecord {
				sys_user: "felix".into(),
				db_user: "tamanu".into(),
				writemode: false,
				ots: None,
				tailscale: vec![TailscalePeer {
					device: "laptop".into(),
					user: "felix@example.com".into(),
				}],
				instance: "7d2c0f4e-1b3a-4c5d-8e9f-0a1b2c3d4e5f".parse().unwrap(),
			}),
		}
	}

	#[test]
	fn context_record_shape() {
		assert_eq!(
			context().to_json().unwrap(),
			r#"{"v":1,"seq":0,"ts":"2026-09-08T03:14:15.926535Z","prev":"","kind":"context","sys_user":"felix","db_user":"tamanu","writemode":false,"ots":null,"tailscale":[{"device":"laptop","user":"felix@example.com"}],"instance":"7d2c0f4e-1b3a-4c5d-8e9f-0a1b2c3d4e5f"}"#
		);
	}

	#[test]
	fn query_record_shape() {
		let record = Record {
			v: FORMAT_VERSION,
			seq: 90,
			ts: "2026-09-08T03:44:09.662377Z".parse().unwrap(),
			prev: "c14e77".into(),
			kind: RecordKind::Query(QueryRecord {
				query: "update patients set updated_at = now() where id = 42;".into(),
				source: QuerySource::Include {
					path: "/home/felix/fixups.sql".into(),
				},
			}),
		};

		assert_eq!(
			record.to_json().unwrap(),
			r#"{"v":1,"seq":90,"ts":"2026-09-08T03:44:09.662377Z","prev":"c14e77","kind":"query","query":"update patients set updated_at = now() where id = 42;","source":"include","path":"/home/felix/fixups.sql"}"#
		);
	}

	#[test]
	fn gap_and_end_record_shapes() {
		let gap = Record {
			v: FORMAT_VERSION,
			seq: 2,
			ts: "2026-09-08T03:44:09.550118Z".parse().unwrap(),
			prev: "e3b0c4".into(),
			kind: RecordKind::Gap(GapRecord {
				lost: 87,
				through: 88,
				from: "2026-09-08T03:14:30.104881Z".parse().unwrap(),
				to: "2026-09-08T03:44:02.771291Z".parse().unwrap(),
			}),
		};
		assert_eq!(
			gap.to_json().unwrap(),
			r#"{"v":1,"seq":2,"ts":"2026-09-08T03:44:09.550118Z","prev":"e3b0c4","kind":"gap","lost":87,"through":88,"from":"2026-09-08T03:14:30.104881Z","to":"2026-09-08T03:44:02.771291Z"}"#
		);

		let end = Record {
			v: FORMAT_VERSION,
			seq: 91,
			ts: "2026-09-08T03:45:01.114202Z".parse().unwrap(),
			prev: "a1d4f0".into(),
			kind: RecordKind::End,
		};
		assert_eq!(
			end.to_json().unwrap(),
			r#"{"v":1,"seq":91,"ts":"2026-09-08T03:45:01.114202Z","prev":"a1d4f0","kind":"end"}"#
		);
	}

	#[test]
	fn every_kind_round_trips() {
		for kind in [
			context().kind,
			RecordKind::Query(QueryRecord {
				query: "select 1;".into(),
				source: QuerySource::Typed,
			}),
			RecordKind::Query(QueryRecord {
				query: "select 2;".into(),
				source: QuerySource::Snippet {
					name: "counts".into(),
				},
			}),
			RecordKind::Query(QueryRecord {
				query: "select 3;".into(),
				source: QuerySource::Unknown,
			}),
			RecordKind::Gap(GapRecord {
				lost: 1,
				through: 5,
				from: Timestamp::UNIX_EPOCH,
				to: Timestamp::UNIX_EPOCH,
			}),
			RecordKind::End,
		] {
			let record = Record { kind, ..context() };
			let json = record.to_json().unwrap();
			assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), record);
		}
	}

	#[test]
	fn separator_never_appears_inside_a_record() {
		let record = Record {
			kind: RecordKind::Query(QueryRecord {
				query: "select '\u{1e}\n\t';".into(),
				source: QuerySource::Typed,
			}),
			..context()
		};
		let json = record.to_json().unwrap();
		assert!(!json.as_bytes().contains(&SEPARATOR));
		assert!(!json.as_bytes().contains(&TERMINATOR));
		assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), record);
	}

	#[test]
	fn framing_surrounds_the_hashed_bytes() {
		let json = context().to_json().unwrap();
		let framed = frame(&json);
		assert_eq!(framed[0], SEPARATOR);
		assert_eq!(*framed.last().unwrap(), TERMINATOR);
		assert_eq!(&framed[1..framed.len() - 1], json.as_bytes());
	}

	#[test]
	fn hash_is_sha256_of_the_json_text() {
		assert_eq!(
			hash(""),
			"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
		);
	}
}
