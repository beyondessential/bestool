use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct StatusResponse {
	pub name: String,
	pub version: String,
	pub started_at: String,
	pub pid: u32,
}
