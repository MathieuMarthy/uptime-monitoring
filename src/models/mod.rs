pub mod admin;
pub mod channel;
pub mod check;
pub mod daily_stat;
pub mod incident;
pub mod monitor;
pub mod monitor_state;
pub mod notification_log;
pub mod session;
pub mod settings;
pub mod stats;
pub mod status_page;

pub use admin::Admin;
pub use channel::{Channel, ChannelSummary};
pub use check::{Check, FailedCheck, RecentCheck};
pub use daily_stat::DailyStat;
pub use incident::{Incident, NamedIncident};
pub use monitor::{Monitor, MonitorSummary};
pub use monitor_state::{CertificateWarning, MonitorState, MonitorStatus, Recorded};
pub use notification_log::{Delivery, LogEntry, NotificationLog};
pub use session::{NewSession, Session};
pub use settings::{Settings, StorageUse};
pub use stats::Bucket;
pub use status_page::{ShownMonitor, StatusPage, StatusPageSummary};

pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}
