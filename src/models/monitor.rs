use crate::models::service::{Service, ServiceStatus};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

const TOTAL_TICKS: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tick {
    pub class: String, // "tick-green", "tick-orange", "tick-red", "tick-gray"
    pub tooltip: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {
    pub service: Service,
    pub last_ping_ms: Option<i64>,
    pub uptime_rate: f64,
    pub ticks: Vec<Tick>,
}

#[derive(Debug, FromRow)]
struct MetricRow {
    service_id: String,
    timestamp: i64,
    response_time_ms: i64,
    status_code: Option<i64>,
}

impl Monitor {
    pub async fn fetch_all(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        let services = Service::find_all(pool).await?;
        if services.is_empty() {
            return Ok(Vec::new());
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let twenty_four_hours_ago = now - (24 * 3600);

        let metrics = sqlx::query_as::<_, MetricRow>(
            r#"
            SELECT service_id, timestamp, response_time_ms, status_code
            FROM metrics
            WHERE timestamp >= ?
            ORDER BY service_id, timestamp ASC
            "#,
        )
        .bind(twenty_four_hours_ago)
        .fetch_all(pool)
        .await?;

        let mut metrics_by_service: HashMap<String, Vec<MetricRow>> = HashMap::new();
        for metric in metrics {
            metrics_by_service
                .entry(metric.service_id.clone())
                .or_default()
                .push(metric);
        }

        let mut monitors = Vec::with_capacity(services.len());

        for service in services {
            let service_metrics = metrics_by_service.remove(&service.id).unwrap_or_default();
            let monitor = Self::compute_monitor(service, service_metrics, now);
            monitors.push(monitor);
        }

        Ok(monitors)
    }

    fn compute_monitor(service: Service, metrics: Vec<MetricRow>, now: i64) -> Self {
        let last_ping_ms = metrics.last().map(|m| m.response_time_ms);

        let total_checks = metrics.len();
        let uptime_rate = if total_checks == 0 {
            if service.current_status == ServiceStatus::Down {
                0.0
            } else {
                100.0
            }
        } else {
            let ok_checks = metrics
                .iter()
                .filter(|m| {
                    m.status_code
                        .map(|code| code == service.expected_status)
                        .unwrap_or(false)
                })
                .count();
            (ok_checks as f64 / total_checks as f64) * 100.0
        };

        let ticks = Self::compute_24h_ticks(&service, &metrics, now);

        Self {
            service,
            last_ping_ms,
            uptime_rate,
            ticks,
        }
    }

    fn compute_24h_ticks(service: &Service, metrics: &[MetricRow], now: i64) -> Vec<Tick> {
        let mut ticks = Vec::with_capacity(TOTAL_TICKS);
        let hour_seconds = 3600;

        for i in 0..TOTAL_TICKS {
            let slot_start = now - ((TOTAL_TICKS - i) as i64 * hour_seconds);
            let slot_end = slot_start + hour_seconds;

            let slot_metrics: Vec<&MetricRow> = metrics
                .iter()
                .filter(|m| m.timestamp >= slot_start && m.timestamp < slot_end)
                .collect();

            let tick = if slot_metrics.is_empty() {
                if slot_end <= service.created_at {
                    Tick {
                        class: "tick-gray".to_string(),
                        tooltip: "Service not created yet".to_string(),
                    }
                } else if !service.is_enabled {
                    Tick {
                        class: "tick-gray".to_string(),
                        tooltip: "Service paused / Not monitored".to_string(),
                    }
                } else {
                    Tick {
                        class: "tick-gray".to_string(),
                        tooltip: "No data recorded during this hour".to_string(),
                    }
                }
            } else {
                let has_down = slot_metrics.iter().any(|m| {
                    m.status_code
                        .map(|code| code != service.expected_status)
                        .unwrap_or(true)
                });

                if has_down {
                    Tick {
                        class: "tick-red".to_string(),
                        tooltip: "Incident reported (Down)".to_string(),
                    }
                } else {
                    let avg_ping: i64 =
                        slot_metrics.iter().map(|m| m.response_time_ms).sum::<i64>()
                            / slot_metrics.len() as i64;

                    if avg_ping > 500 {
                        Tick {
                            class: "tick-orange".to_string(),
                            tooltip: format!("Degraded performance (avg: {}ms)", avg_ping),
                        }
                    } else {
                        Tick {
                            class: "tick-green".to_string(),
                            tooltip: format!("Operational (avg: {}ms)", avg_ping),
                        }
                    }
                }
            };

            ticks.push(tick);
        }

        ticks
    }
}
