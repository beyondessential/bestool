//! Bind spawned child processes' lifetime to the daemon's.
//!
//! Backups spawn helpers (`pg_basebackup`, `kopia`) as child processes. If the
//! daemon exits mid-backup — a clean stop, a crash, an OOM kill, or a self-update
//! restart — those children must go with it. On Linux systemd's cgroup tears the
//! whole unit down, so this is handled there. On Windows nothing ties a child to
//! its parent by default: a stranded `pg_basebackup` keeps streaming into the
//! stable staging dir, and the run the restarted daemon starts collides with it
//! (`could not create directory ...: File exists`). A job object with
//! kill-on-close makes every child die when the daemon process does.
//!
//! Job membership is inherited by every descendant, and can't be shaken off by
//! exiting the intermediate parent, so the job also captures processes the doctor
//! merely probes. That is wrong for pm2: its CLI is connect-or-launch, so a
//! `pm2 jlist` that can't reach the God daemon starts one inside the job, and
//! from then on every Tamanu service that daemon supervises dies whenever alertd
//! exits — a clean stop, a crash, a watchdog trip, or a self-update restart. The
//! job therefore permits breakaway, and the pm2 module opts its own invocations
//! out (see [`bestool_tamanu::pm2::allow_job_breakaway`]).

/// Confine every process the daemon spawns to the daemon's own lifetime, so none
/// outlive it. A no-op on platforms where the service manager already guarantees
/// this (Linux under systemd).
pub fn confine_children() {
	#[cfg(windows)]
	imp::confine();
}

#[cfg(windows)]
mod imp {
	use tracing::{info, warn};
	use win32job::{ExtendedLimitInfo, Job};

	pub fn confine() {
		let mut info = ExtendedLimitInfo::new();
		info.limit_kill_on_job_close();
		// Without this, the CREATE_BREAKAWAY_FROM_JOB the pm2 module needs fails
		// the spawn outright with ERROR_ACCESS_DENIED. Only pm2 uses it; every
		// other child stays confined.
		info.limit_breakaway_ok();

		let job = match Job::create_with_limit_info(&info) {
			Ok(job) => job,
			Err(err) => {
				warn!(
					"could not create the job object to confine backup children ({err}); \
					 they may outlive a daemon restart"
				);
				return;
			}
		};

		if let Err(err) = job.assign_current_process() {
			warn!(
				"could not assign the daemon to its job object ({err}); \
				 backup children may outlive a daemon restart"
			);
			return;
		}

		// Keep the handle open for the daemon's whole life: closing it is what fires
		// the kill, so it must outlive every backup. `into_handle` yields the raw
		// handle without running `Drop` (which would close it here and now).
		let _ = job.into_handle();

		// Only now that we're actually in a job that permits breakaway is it safe
		// for pm2 to ask for it.
		bestool_tamanu::pm2::allow_job_breakaway();

		info!("confined child processes to the daemon's lifetime");
	}
}
