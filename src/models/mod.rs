pub mod monitor;
pub mod monitor_state;

pub use monitor::Monitor;
pub use monitor_state::{MonitorState, MonitorStatus};

pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}
