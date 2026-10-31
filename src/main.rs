use std::io::IsTerminal;
use std::net::SocketAddr;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use uptime_monitoring::config::Config;
use uptime_monitoring::engine::guard::Guard;
use uptime_monitoring::models::{Admin, Settings};
use uptime_monitoring::notify::Notifier;
use uptime_monitoring::state::AppState;
use uptime_monitoring::windows::Maintenances;
use uptime_monitoring::{app, db, engine, healthcheck, maintenance, privileges};

fn main() {
    let command = std::env::args().nth(1);
    let command = match command.as_deref() {
        None => Command::Serve,
        Some("healthcheck") => Command::Healthcheck,
        Some("disable-totp") => Command::DisableTotp,
        Some(other) => {
            eprintln!(
                "Unknown command {other:?}. Run without arguments, or with `healthcheck` or `disable-totp`."
            );
            std::process::exit(2);
        }
    };
    let serving = matches!(command, Command::Serve);

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("Invalid configuration: {message}");
            std::process::exit(1);
        }
    };

    // Before any thread, file or socket: root only lasts long enough to hand over the database.
    if privileges::is_root() {
        let (uid, gid) = (config.puid, config.pgid);
        let handed = match serving {
            true => privileges::hand_over_database(&config.database_url, uid, gid),
            false => Ok(()),
        };
        if let Err(e) = handed.and_then(|()| privileges::become_user(uid, gid)) {
            eprintln!("Cannot switch from root to user {uid}:{gid} (PUID, PGID): {e}");
            std::process::exit(1);
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to start the async runtime");
    match command {
        Command::Serve => runtime.block_on(serve(config)),
        Command::Healthcheck => runtime.block_on(healthcheck(config)),
        Command::DisableTotp => runtime.block_on(disable_totp(config)),
    }
}

enum Command {
    Serve,
    Healthcheck,
    /// Recovery when the authenticator app is lost: needs access to the server.
    DisableTotp,
}

async fn serve(config: Config) {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "uptime_monitoring=info,tower_http=info".into()),
        )
        // No colour codes in `docker logs` or a log file.
        .with(tracing_subscriber::fmt::layer().with_ansi(std::io::stdout().is_terminal()))
        .init();

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

    let guard = match Settings::load(&pool).await {
        Ok(settings) => Guard::new(settings.block_private_targets),
        Err(e) => {
            eprintln!("Cannot read the settings: {e}");
            std::process::exit(1);
        }
    };
    let maintenances = Maintenances::default();
    if let Err(e) = maintenances.reload(&pool).await {
        eprintln!("Cannot read the maintenance windows: {e}");
        std::process::exit(1);
    }
    let notifier = Notifier::start(pool.clone(), guard.clone(), config.public_url.clone());
    let engine = match engine::start(
        pool.clone(),
        guard.clone(),
        notifier.clone(),
        maintenances.clone(),
    )
    .await
    {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("Cannot start the check engine: {e}");
            std::process::exit(1);
        }
    };

    maintenance::start(pool.clone());

    let state = AppState::new(pool)
        .with_trust_proxy_headers(config.trust_proxy_headers)
        .with_public_url(config.public_url.clone())
        .with_engine(engine)
        .with_maintenances(maintenances)
        .with_guard(guard)
        .with_notifier(notifier);
    match state.start_setup_if_needed().await {
        Ok(Some(token)) => {
            uptime_monitoring::auth::print_setup_url(&token, config.public_url.as_deref())
        }
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

/// Exits 0 when the instance at `BIND_ADDR` answers `/health`, 1 otherwise.
async fn healthcheck(config: Config) -> ! {
    match healthcheck::probe(healthcheck::target(config.bind_addr)).await {
        Ok(()) => std::process::exit(0),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

async fn disable_totp(config: Config) -> ! {
    let result = async {
        let pool = db::init_pool(&config.database_url).await?;
        Admin::disable_totp(&pool).await
    };
    match result.await {
        Ok(()) => {
            eprintln!("Two-factor authentication is off; log in with the password alone.");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Cannot turn two-factor authentication off: {e}");
            std::process::exit(1);
        }
    }
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
