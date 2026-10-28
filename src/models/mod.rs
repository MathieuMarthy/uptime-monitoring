pub mod incident;
pub mod monitor;
pub mod monitor_state;
pub mod service;

pub use incident::Incident;
pub use monitor::Monitor;
pub use monitor_state::{MonitorState, MonitorStatus};
pub use service::{Service, ServiceFormInput, ServiceStatus};

pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}
