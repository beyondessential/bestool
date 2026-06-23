mod control;
mod health;
mod index;
mod metrics;
mod status;
mod tasks;

pub use control::{handle_reload, handle_restart};
pub use health::handle_health;
pub use index::handle_index;
pub use metrics::handle_metrics;
pub use status::handle_status;
pub use tasks::handle_task_endpoint;
