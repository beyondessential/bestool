mod alert;
mod alerts;
mod index;
mod metrics;
mod pause_alert;
mod reload;
mod status;
mod targets;
mod validate;

pub use alert::handle_alert;
pub use alerts::handle_alerts;
pub use index::handle_index;
pub use metrics::handle_metrics;
pub use pause_alert::handle_pause_alert;
pub use reload::handle_reload;
pub use status::handle_status;
pub use targets::handle_targets;
pub use validate::handle_validate;
