pub mod admin;
pub mod check;
pub mod incident;
pub mod monitor;
pub mod monitor_state;
pub mod session;

pub use admin::Admin;
pub use check::Check;
pub use incident::Incident;
pub use monitor::Monitor;
pub use monitor_state::{MonitorState, MonitorStatus};
pub use session::{NewSession, Session};

pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}
