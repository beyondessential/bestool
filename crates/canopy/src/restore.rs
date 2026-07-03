//! Request bodies for the managed-restore endpoints that canopy's OpenAPI
//! document doesn't expose as named schemas.
//!
//! The restore responses and the verification report are generated wire types —
//! see [`schema::WorklistEntry`](crate::schema::WorklistEntry),
//! [`schema::RestoreCredentials`](crate::schema::RestoreCredentials), and
//! [`schema::VerificationArgs`](crate::schema::VerificationArgs). Only the two
//! request bodies below live here, because canopy declares them inline rather
//! than as reusable schema components.

use serde::Serialize;
use uuid::Uuid;

/// Body for `POST /restore-capabilities`: the restore intents this consumer supports.
///
/// Replaces the registered intent set wholesale; canopy dispatches only matching
/// worklist entries.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreCapabilitiesRequest<'a> {
	pub intents: &'a [&'a str],
}

/// Body for `POST /restore-credentials`.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreCredentialsRequest<'a> {
	pub group: Uuid,
	pub r#type: &'a str,
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	#[test]
	fn capabilities_request_lists_intents() {
		let intents = ["verify", "analytics", "disaster-recovery"];
		let req = RestoreCapabilitiesRequest { intents: &intents };
		assert_eq!(
			serde_json::to_value(&req).unwrap(),
			json!({"intents": ["verify", "analytics", "disaster-recovery"]})
		);
	}

	#[test]
	fn credentials_request_carries_group_and_type() {
		let group = "11111111-1111-1111-1111-111111111111".parse().unwrap();
		let req = RestoreCredentialsRequest {
			group,
			r#type: "tamanu-postgres",
		};
		assert_eq!(
			serde_json::to_value(&req).unwrap(),
			json!({
				"group": "11111111-1111-1111-1111-111111111111",
				"type": "tamanu-postgres",
			})
		);
	}
}
