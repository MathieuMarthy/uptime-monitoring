use std::net::SocketAddr;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use uptime_monitoring::config::Config;
use uptime_monitoring::state::AppState;
use uptime_monitoring::{app, db, engine, maintenance};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "uptime_monitoring=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("Invalid configuration: {message}");
            std::process::exit(1);
        }
    };

    let pool = match db::init_pool(&config.database_url).await {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!(
                "Cannot open the database at DATABASE_URL={:?}: {e}",
                config.database_url
            );
            std::process::exit(1);
        }
    };

    let engine = match engine::start(pool.clone()).await {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("Cannot start the check engine: {e}");
            std::process::exit(1);
        }
    };

    maintenance::start(pool.clone());

    let state = AppState::new(pool)
        .with_trust_proxy_headers(config.trust_proxy_headers)
        .with_engine(engine);
    match state.start_setup_if_needed().await {
        Ok(Some(token)) => uptime_monitoring::auth::print_setup_url(&token),
        Ok(None) => {}
        Err(e) => {
            eprintln!("Cannot read the admin account: {e}");
            std::process::exit(1);
        }
    }

    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .expect("Failed to bind address");
    tracing::info!(
        "uptime-monitoring is running on http://{}",
        config.bind_addr
    );
    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("Server error");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => tracing::info!("Received Ctrl+C, starting graceful shutdown..."),
        _ = terminate => tracing::info!("Received SIGTERM, starting graceful shutdown..."),
    }
}
