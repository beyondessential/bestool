//! How a check reads the runtime of the application it reports for.
//!
//! A substrate covers only what genuinely differs between environments: the
//! services running an application, the traffic reaching it, and the
//! certificates in front of it. What a check needs that does not differ — a
//! database connection, a configuration, a version — is a parameter on its
//! context rather than a reading taken through here.
//!
//! The readings divide by what an application is, so there are two traits
//! rather than one. Every application is run by something, so every one has
//! [`ServiceRuntime`]. Only an application that serves HTTP has traffic or
//! certificates, so only those carry [`HttpRuntime`]; a Postgres cluster is not
//! asked for its traffic and so never reports it as unavailable.
//!
//! Machine checks read the host directly and use no substrate at all.
//!
//! spec: SUB

use std::{collections::BTreeMap, fmt};

use async_trait::async_trait;

pub mod caddy;
pub mod pg;
pub mod pm2;
pub mod systemd;

/// Why a reading could not be taken.
///
/// Free-form for now: there is not enough usage to know which causes are real,
/// so a check turns this straight into the reason of its skip and the closed
/// set — not permitted, not reachable, not present — is derived later from what
/// actually gets written.
///
/// spec: SUB
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable(String);

impl Unavailable {
	pub fn new(reason: impl Into<String>) -> Self {
		Self(reason.into())
	}

	pub fn reason(&self) -> &str {
		&self.0
	}
}

impl fmt::Display for Unavailable {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

impl std::error::Error for Unavailable {}

/// Whether an application's compute is switched on.
///
/// An application can exist and hold its data while all of its compute is off,
/// which is the intended condition rather than a fault: it has no running
/// services and no reachable database, and the checks that need either skip
/// giving that as the reason.
///
/// spec: SUB
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compute {
	Running,
	SwitchedOff,
}

impl Compute {
	pub fn is_switched_off(self) -> bool {
		matches!(self, Self::SwitchedOff)
	}
}

/// The job a service does for Tamanu.
///
/// No central-or-facility distinction: a duty's job is the same whichever kind
/// of server runs it, and which kinds run which duties changes over time. The
/// role an application plays is carried by its type, so it is stated once for
/// the application rather than encoded into each duty's name.
///
/// spec: SUB#the-duty-vocabulary
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TamanuDuty {
	Api,
	Tasks,
	Sync,
	Frontend,
	FhirResolve,
	FhirRefresh,
	PatientPortal,
}

impl TamanuDuty {
	pub const ALL: [Self; 7] = [
		Self::Api,
		Self::Tasks,
		Self::Sync,
		Self::Frontend,
		Self::FhirResolve,
		Self::FhirRefresh,
		Self::PatientPortal,
	];

	pub fn as_str(self) -> &'static str {
		match self {
			Self::Api => "api",
			Self::Tasks => "tasks",
			Self::Sync => "sync",
			Self::Frontend => "frontend",
			Self::FhirResolve => "fhir-resolve",
			Self::FhirRefresh => "fhir-refresh",
			Self::PatientPortal => "patient-portal",
		}
	}
}

/// The job a service does for a Postgres installation.
///
/// A Postgres installation is an application in its own right rather than a
/// duty of whatever uses it, so its services are its own: a single server on a
/// machine, or a primary alongside its replicas on a substrate that runs it
/// that way.
///
/// spec: SUB#the-duty-vocabulary
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PostgresDuty {
	/// The server accepting writes. A lone server on a machine is this.
	Primary,
	Replica,
}

impl PostgresDuty {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Primary => "primary",
			Self::Replica => "replica",
		}
	}
}

/// What job a service does, named the same way on every substrate.
///
/// A check reads a duty rather than a unit name, a process name or a pod name,
/// so the same grading runs wherever the application does.
///
/// The vocabulary is organised by product, so products whose duties have
/// nothing in common never share a set of names. Tamanu is the only product it
/// covers, and it is shaped to admit others without that changing.
///
/// spec: SUB#the-duty-vocabulary
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Duty {
	Tamanu(TamanuDuty),
	Postgres(PostgresDuty),
	/// A service whose duty is outside the vocabulary, carried under the name
	/// it was found by rather than dropped.
	///
	/// This is also how a deployment shape that should no longer exist stays
	/// visible: a check that grades such a service as forbidden finds it here,
	/// without the shared vocabulary having to carry a duty nothing should be
	/// running.
	Other(String),
}

impl Duty {
	/// The duty of a Tamanu service found under `name`.
	///
	/// Both supervisors name their services `tamanu-${thing}`, systemd
	/// interposing the server's role (`tamanu-central-api`, `tamanu-facility-
	/// sync`) where pm2 does not (`tamanu-api`). The role is dropped: it is the
	/// application's, not the duty's.
	///
	/// A name the vocabulary does not cover comes back as [`Duty::Other`]
	/// carrying the name as found, which is what keeps the legacy
	/// `tamanu-facility` singleton visible to the check that forbids it.
	pub fn from_tamanu_service_name(name: &str) -> Self {
		let job = name
			.strip_prefix("tamanu-")
			.map(|rest| {
				rest.strip_prefix("central-")
					.or_else(|| rest.strip_prefix("facility-"))
					.unwrap_or(rest)
			})
			.unwrap_or(name);

		match job {
			"api" => Self::Tamanu(TamanuDuty::Api),
			"tasks" => Self::Tamanu(TamanuDuty::Tasks),
			"sync" => Self::Tamanu(TamanuDuty::Sync),
			"frontend" => Self::Tamanu(TamanuDuty::Frontend),
			"fhir-resolve" => Self::Tamanu(TamanuDuty::FhirResolve),
			"fhir-refresh" => Self::Tamanu(TamanuDuty::FhirRefresh),
			"patientportal" | "patient-portal" => Self::Tamanu(TamanuDuty::PatientPortal),
			_ => Self::Other(name.to_string()),
		}
	}

	/// Whether this duty is outside the shared vocabulary.
	pub fn is_other(&self) -> bool {
		matches!(self, Self::Other(_))
	}
}

impl fmt::Display for Duty {
	/// The job alone, without the product: which product a duty belongs to is
	/// already carried by the application's type, so repeating it in every
	/// metric label and diagnostic would say nothing.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Tamanu(duty) => f.write_str(duty.as_str()),
			Self::Postgres(duty) => f.write_str(duty.as_str()),
			Self::Other(name) => f.write_str(name),
		}
	}
}

/// How a check names one service back to the substrate it came from.
///
/// Opaque: a unit name on systemd, a process name and id on pm2, a pod on
/// Kubernetes. A check passes it back to ask for that service's facts and
/// otherwise only shows it in diagnostics.
///
/// spec: SUB#the-workload
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServiceId(String);

impl ServiceId {
	pub fn new(id: impl Into<String>) -> Self {
		Self(id.into())
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}
}

impl fmt::Display for ServiceId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

/// One of the services making up an application.
///
/// A service is one container, one supervised process, or one pod. Several
/// commonly share a duty: an API duty usually runs more than one, and a
/// frontend duty runs a named instance per slot.
///
/// spec: SUB#the-workload
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
	/// What a check passes back to ask for this service's facts.
	pub id: ServiceId,
	pub duty: Duty,
	/// Which slot this service occupies within its duty, where the runtime
	/// names them — a systemd template instance, a pod's ordinal. `None` where
	/// the runtime runs a duty's services unnamed, which is how pm2 runs a
	/// clustered process and how a singleton runs everywhere.
	pub slot: Option<String>,
	/// Whether the runtime will bring this service up of its own accord: a
	/// systemd unit that is enabled, a pm2 process in the saved process list, a
	/// Kubernetes workload asking for a replica.
	///
	/// Separate from whether it is up, so a service that is configured to run
	/// and is not can be told from one nothing intends to run at all.
	pub scheduled: bool,
}

/// What a substrate can answer about one service.
///
/// spec: SUB#service-facts
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceFacts {
	/// Whether the service is up right now.
	pub up: bool,
	/// The version it is running, resolved by the substrate from whatever names
	/// it there — an image tag or label on a container runtime, the install's
	/// version where every process necessarily shares one.
	///
	/// Three answers, because a check grading drift has to tell them apart:
	/// `Ok(Some)` is a version, `Ok(None)` is a runtime that names none, and
	/// `Err` is one that could not be asked. A check that took the last for the
	/// middle would report a blind sweep as a clean one.
	pub version: Result<Option<String>, Unavailable>,
	/// Memory in use, in bytes.
	pub memory_bytes: Option<u64>,
	/// The memory ceiling declared for this service specifically, in bytes: a
	/// container's memory limit, a Kubernetes container limit, or the memory
	/// bounds configured on a supervised unit.
	///
	/// `None` where the service declares none, in which case there is no
	/// denominator to take a percentage of and grading skips for it. Never the
	/// machine's total, which is shared with everything else on it and says
	/// nothing about whether a service is near its own limit.
	pub memory_ceiling_bytes: Option<u64>,
	/// Processor time consumed since the service started, in seconds.
	///
	/// Cumulative rather than a rate: two readings a sweep apart give the rate,
	/// and a service that restarts resets to zero rather than reporting a rate
	/// nothing measured.
	pub processor_seconds: Option<f64>,
}

impl Default for ServiceFacts {
	/// Nothing known about a service, which is what a runtime answers for one
	/// it can name but not describe.
	fn default() -> Self {
		Self {
			up: false,
			version: Ok(None),
			memory_bytes: None,
			memory_ceiling_bytes: None,
			processor_seconds: None,
		}
	}
}

/// Cumulative HTTP request counts for an application.
///
/// Kept per source because the counters behind them are per front end and those
/// roll: a source that has vanished is dropped rather than its disappearance
/// being graded as the quantity having fallen.
///
/// spec: SUB#http-traffic-and-certificates
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrafficCounters {
	pub sources: Vec<TrafficSource>,
}

/// One front end's cumulative request counts, for the application being
/// reported for.
///
/// Where an application fronts its own traffic there is one source covering the
/// whole machine. Where traffic is served by shared infrastructure the counts
/// are filtered to the application, so one scrape serves each application
/// behind it separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficSource {
	/// What produced these counts, stable for as long as it exists: a front end
	/// instance, a gateway, a pod.
	pub source: String,
	/// Requests counted per HTTP status code, since the source started.
	pub by_status: BTreeMap<String, u64>,
}

/// What is running an application. Every application has one.
///
/// spec: SUB
#[async_trait]
pub trait ServiceRuntime: Send + Sync {
	/// Whether the application's compute is switched on.
	async fn compute(&self) -> Compute;

	/// The services making up the application.
	async fn services(&self) -> Result<Vec<Service>, Unavailable>;

	/// One service's facts, by the identifier [`services`](Self::services) gave
	/// for it.
	async fn service_facts(&self, id: &ServiceId) -> Result<ServiceFacts, Unavailable>;
}

/// A runtime for a platform none of the own-system implementations covers.
///
/// Answers every reading as unavailable with the one reason, so a check that
/// needs one skips saying the substrate cannot serve it. That is the honest
/// reading: an application does run somewhere, and this is a host bestool does
/// not know how to look.
pub struct Unsupported {
	reason: String,
}

impl Unsupported {
	pub fn new(reason: impl Into<String>) -> Self {
		Self {
			reason: reason.into(),
		}
	}
}

#[async_trait]
impl ServiceRuntime for Unsupported {
	async fn compute(&self) -> Compute {
		Compute::Running
	}

	async fn services(&self) -> Result<Vec<Service>, Unavailable> {
		Err(Unavailable::new(self.reason.clone()))
	}

	async fn service_facts(&self, _id: &ServiceId) -> Result<ServiceFacts, Unavailable> {
		Err(Unavailable::new(self.reason.clone()))
	}
}

/// What reaches an application that serves HTTP, and what fronts it.
///
/// Two fields rather than a supertrait of [`ServiceRuntime`]: the readings come
/// from genuinely different sources — services from a supervisor and traffic
/// from the front end on a machine, the cluster API and the gateway on
/// Kubernetes — so one object implementing both would make every implementer
/// compose two unrelated things for no benefit at the call site.
///
/// The certificates in force for an application belong here too, and join this
/// when the check that grades them reads them through a substrate; for now it
/// reads the front end directly.
///
/// spec: SUB#http-traffic-and-certificates
#[async_trait]
pub trait HttpRuntime: Send + Sync {
	/// Cumulative request counts for this application, per source.
	async fn http_counters(&self) -> Result<TrafficCounters, Unavailable>;
}

#[cfg(test)]
pub mod fake {
	//! A runtime that answers with whatever a test scripted, so a check's
	//! grading can be exercised without a supervisor under it.

	use std::collections::HashMap;

	use super::*;

	pub struct FakeRuntime {
		pub compute: Option<Compute>,
		pub services: Result<Vec<Service>, Unavailable>,
		pub facts: HashMap<ServiceId, ServiceFacts>,
	}

	impl FakeRuntime {
		/// A runtime running nothing, which is what most contexts want: the
		/// check under test is not the one reading services.
		pub fn empty() -> Self {
			Self {
				compute: Some(Compute::Running),
				services: Ok(Vec::new()),
				facts: HashMap::new(),
			}
		}

		/// A runtime that cannot be read at all.
		pub fn unavailable(reason: &str) -> Self {
			Self {
				compute: Some(Compute::Running),
				services: Err(Unavailable::new(reason)),
				facts: HashMap::new(),
			}
		}

		/// Add a service, with the facts it answers for itself.
		pub fn with(mut self, service: Service, facts: ServiceFacts) -> Self {
			self.facts.insert(service.id.clone(), facts);
			self.services
				.as_mut()
				.expect("cannot add a service to an unreadable runtime")
				.push(service);
			self
		}
	}

	/// A traffic runtime that answers with whatever a test scripted.
	pub struct FakeTraffic {
		pub counters: Result<TrafficCounters, Unavailable>,
	}

	impl FakeTraffic {
		/// A front end that is not there, which is what a host with none gives
		/// and what most contexts want: the check under test is not the one
		/// reading traffic.
		pub fn absent() -> Self {
			Self::unavailable("no front end on this host")
		}

		/// A front end that is there and has served nothing.
		pub fn quiet() -> Self {
			Self {
				counters: Ok(TrafficCounters::default()),
			}
		}

		/// Counts from one source, as a machine's own front end reports them.
		pub fn from_one_source(source: &str, by_status: &[(&str, u64)]) -> Self {
			Self {
				counters: Ok(TrafficCounters {
					sources: vec![TrafficSource {
						source: source.to_string(),
						by_status: by_status
							.iter()
							.map(|(code, n)| ((*code).to_string(), *n))
							.collect(),
					}],
				}),
			}
		}

		pub fn unavailable(reason: &str) -> Self {
			Self {
				counters: Err(Unavailable::new(reason)),
			}
		}
	}

	#[async_trait]
	impl HttpRuntime for FakeTraffic {
		async fn http_counters(&self) -> Result<TrafficCounters, Unavailable> {
			self.counters.clone()
		}
	}

	#[async_trait]
	impl ServiceRuntime for FakeRuntime {
		async fn compute(&self) -> Compute {
			self.compute.unwrap_or(Compute::Running)
		}

		async fn services(&self) -> Result<Vec<Service>, Unavailable> {
			self.services.clone()
		}

		async fn service_facts(&self, id: &ServiceId) -> Result<ServiceFacts, Unavailable> {
			self.facts
				.get(id)
				.cloned()
				.ok_or_else(|| Unavailable::new(format!("no service {id}")))
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Both supervisors name the same job the same duty, so a check grading a
	/// duty grades the same thing on either. The role systemd interposes is the
	/// application's, not the duty's, and is dropped.
	///
	/// spec: SUB#the-duty-vocabulary
	#[test]
	fn a_duty_is_the_same_on_either_supervisor() {
		for (systemd, pm2, duty) in [
			("tamanu-central-api", "tamanu-api", TamanuDuty::Api),
			("tamanu-facility-api", "tamanu-api", TamanuDuty::Api),
			("tamanu-central-tasks", "tamanu-tasks", TamanuDuty::Tasks),
			("tamanu-facility-sync", "tamanu-sync", TamanuDuty::Sync),
			(
				"tamanu-central-fhir-resolve",
				"tamanu-fhir-resolve",
				TamanuDuty::FhirResolve,
			),
			(
				"tamanu-central-fhir-refresh",
				"tamanu-fhir-refresh",
				TamanuDuty::FhirRefresh,
			),
		] {
			assert_eq!(
				Duty::from_tamanu_service_name(systemd),
				Duty::Tamanu(duty),
				"{systemd} should read as {duty:?}"
			);
			assert_eq!(
				Duty::from_tamanu_service_name(pm2),
				Duty::Tamanu(duty),
				"{pm2} should read as {duty:?}"
			);
		}
	}

	/// The duties that carry no role on either supervisor.
	#[test]
	fn roleless_duties_read_straight_through() {
		assert_eq!(
			Duty::from_tamanu_service_name("tamanu-frontend"),
			Duty::Tamanu(TamanuDuty::Frontend)
		);
		assert_eq!(
			Duty::from_tamanu_service_name("tamanu-patientportal"),
			Duty::Tamanu(TamanuDuty::PatientPortal)
		);
	}

	/// A service the vocabulary does not cover keeps the name it was found by,
	/// which is what lets a check forbid the legacy singleton without the
	/// vocabulary carrying a duty nothing should be running.
	///
	/// spec: SUB#the-duty-vocabulary
	#[test]
	fn an_unknown_service_keeps_its_found_name() {
		assert_eq!(
			Duty::from_tamanu_service_name("tamanu-facility"),
			Duty::Other("tamanu-facility".into())
		);
		assert_eq!(
			Duty::from_tamanu_service_name("caddy"),
			Duty::Other("caddy".into())
		);
	}

	/// Postgres is an application in its own right, so it is never a Tamanu
	/// duty however it is named.
	///
	/// spec: SUB#the-duty-vocabulary
	#[test]
	fn postgres_is_not_a_tamanu_duty() {
		assert!(matches!(
			Duty::from_tamanu_service_name("tamanu-postgres"),
			Duty::Other(_)
		));
		assert!(
			!TamanuDuty::ALL
				.iter()
				.any(|d| d.as_str() == PostgresDuty::Primary.as_str())
		);
	}

	/// A duty renders as its job alone: the product is already carried by the
	/// application's type, so a metric label repeating it would say nothing.
	#[test]
	fn a_duty_renders_as_its_job() {
		assert_eq!(
			Duty::Tamanu(TamanuDuty::FhirResolve).to_string(),
			"fhir-resolve"
		);
		assert_eq!(Duty::Postgres(PostgresDuty::Primary).to_string(), "primary");
		assert_eq!(
			Duty::Other("tamanu-facility".into()).to_string(),
			"tamanu-facility"
		);
	}
}
