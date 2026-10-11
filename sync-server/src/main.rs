use std::sync::{Arc, Mutex};

use sync_server::config::Config;
use sync_server::{db, routes, users};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => {
            dotenvy::dotenv().ok();
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .init();
            let cfg = Config::from_env()?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(serve(cfg))
        }
        Some("add-user") => {
            let (email, code) = parse_add_user_args(&args)?;
            account_op(|conn| {
                let created = users::add_user(conn, &email, code)?;
                println!("created {}\nlogin code: {}", created.email, created.code);
                println!("store it now — it is not recoverable later");
                Ok(())
            })
        }
        Some("list-users") => account_op(|conn| {
            for row in users::list_users(conn)? {
                println!("{}", users::format_user_row(&row));
            }
            Ok(())
        }),
        Some("disable-user") | Some("enable-user") => {
            let enable = args[0] == "enable-user";
            let email = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("usage: {} <email>", args[0]))?;
            account_op(|conn| {
                if users::set_disabled(conn, email, !enable)? {
                    println!("{email} {}", if enable { "enabled" } else { "disabled" });
                } else {
                    eprintln!("no such user: {email}");
                    std::process::exit(1);
                }
                Ok(())
            })
        }
        Some("passwd") => {
            let email = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("usage: passwd <email> [--code <new-code>]"))?;
            let code = args
                .iter()
                .position(|a| a == "--code")
                .and_then(|i| args.get(i + 1))
                .cloned();
            account_op(|conn| match users::reset_code(conn, email, code)? {
                Some(code) => {
                    println!("new login code for {email}: {code}");
                    Ok(())
                }
                None => {
                    eprintln!("no such user: {email}");
                    std::process::exit(1);
                }
            })
        }
        Some(other) => {
            eprintln!(
                "unknown subcommand: {other}\nusage: sync-server [serve | add-user <email> [--code X] | list-users | disable-user <email> | enable-user <email> | passwd <email> [--code X]]"
            );
            std::process::exit(2);
        }
    }
}

fn parse_add_user_args(args: &[String]) -> anyhow::Result<(String, Option<String>)> {
    let email = args
        .get(1)
        .filter(|a| !a.starts_with("--"))
        .ok_or_else(|| anyhow::anyhow!("usage: add-user <email> [--code X]"))?;
    let code = args
        .iter()
        .position(|a| a == "--code")
        .and_then(|i| args.get(i + 1))
        .cloned();
    Ok((email.clone(), code))
}

/// Account commands share the server's env/DB but never need JWT_SECRET.
fn account_op(
    f: impl FnOnce(&mut rusqlite::Connection) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cfg = Config::from_env_base();
    let mut conn = db::open_db(&cfg)?;
    f(&mut conn)
}

async fn serve(cfg: Config) -> anyhow::Result<()> {
    std::fs::create_dir_all(cfg.files_root())?;
    let removed = sync_server::storage::gc_blobs(&cfg);
    if removed > 0 {
        tracing::info!("blob gc: removed {removed} orphaned content-addressed blob(s)");
    }
    let conn = db::open_db(&cfg)?;
    let state: db::SharedState = Arc::new(db::AppState {
        db: conn.into(),
        cfg,
        login_limiter: Mutex::new(sync_server::ratelimit::LoginLimiter::new()),
    });

    let addr = state.cfg.listen_addr.clone();
    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("sync-server listening on {addr}");
    // ConnectInfo supplies the client IP to the login rate limiter.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
