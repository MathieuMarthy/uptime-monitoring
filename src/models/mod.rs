pub mod admin;
pub mod check;
pub mod daily_stat;
pub mod incident;
pub mod monitor;
pub mod monitor_state;
pub mod session;
pub mod settings;
pub mod stats;

pub use admin::Admin;
pub use check::Check;
pub use daily_stat::DailyStat;
pub use incident::Incident;
pub use monitor::Monitor;
pub use monitor_state::{MonitorState, MonitorStatus};
pub use session::{NewSession, Session};
pub use settings::Settings;
pub use stats::Bucket;

pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}
