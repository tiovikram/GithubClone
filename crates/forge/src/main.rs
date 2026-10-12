use clap::{Parser, Subcommand};
use forge::config::Config;

#[derive(Parser)]
#[command(name = "forge", about = "GitHub-compatible code forge")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the HTTP server (default).
    Serve,
    /// Apply database migrations and exit.
    Migrate,
    /// Repack repositories and delete unreferenced objects.
    Gc,
    /// Administrative commands.
    Admin {
        #[command(subcommand)]
        cmd: AdminCmd,
    },
}

#[derive(Subcommand)]
enum AdminCmd {
    CreateUser {
        #[arg(long)]
        login: String,
        #[arg(long)]
        password: String,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        admin: bool,
    },
    CreateToken {
        #[arg(long)]
        login: String,
        #[arg(long, default_value = "cli")]
        name: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "forge=info,tower_http=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let cfg = Config::from_env();
    match cli.cmd.unwrap_or(Cmd::Serve) {
        Cmd::Serve => {
            let listen = cfg.listen.clone();
            let state = forge::build_state(cfg).await?;
            let app = forge::app(state);
            // Optional unix socket listener (used by gh's `http_unix_socket` in tests).
            if let Ok(sock) = std::env::var("FORGE_UNIX_SOCKET") {
                let _ = std::fs::remove_file(&sock);
                let ul = tokio::net::UnixListener::bind(&sock)?;
                tracing::info!("forge listening on unix:{sock}");
                let app = app.clone();
                tokio::spawn(async move {
                    if let Err(e) = axum::serve(ul, app).await {
                        tracing::error!(error = %e, "unix listener failed");
                    }
                });
            }
            // TCP_NODELAY: git smart-HTTP responses are streamed from a child process in small
            // chunks; with Nagle + the client's delayed ACK every such response stalled ~40ms.
            let listener = axum::serve::ListenerExt::tap_io(tokio::net::TcpListener::bind(&listen).await?, |tcp| {
                let _ = tcp.set_nodelay(true);
            });
            tracing::info!("forge listening on {listen}");
            axum::serve(listener, app).with_graceful_shutdown(shutdown()).await?;
        }
        Cmd::Migrate => {
            let state = forge::build_state(cfg).await?;
            forge::migrate(&state.db).await?;
        }
        Cmd::Gc => {
            let state = forge::build_state(cfg).await?;
            forge::git::gc::run(&state).await?;
        }
        Cmd::Admin { cmd } => {
            let state = forge::build_state(cfg).await?;
            match cmd {
                AdminCmd::CreateUser { login, password, email, admin } => {
                    let u = forge::forge_api::create_user(&state, &login, email.as_deref(), &password, admin)
                        .await
                        .map_err(|e| anyhow::anyhow!("{}: {:?}", e.message, e.errors))?;
                    println!("created user {} (id {})", u.login, u.id);
                }
                AdminCmd::CreateToken { login, name } => {
                    let u = forge::db::user_by_login(&state.db, &login).await.map_err(|e| anyhow::anyhow!(e.message))?;
                    let (_, token) = forge::forge_api::create_token_for(&state, u.id, &name)
                        .await
                        .map_err(|e| anyhow::anyhow!(e.message))?;
                    println!("{token}");
                }
            }
        }
    }
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    tracing::info!("shutting down");
}
