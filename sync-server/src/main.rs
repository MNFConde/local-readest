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
            let (email, code, role) = parse_add_user_args(&args)?;
            account_op(|conn| {
                let created = users::add_user(conn, &email, code, &role)?;
                if created.code.is_empty() {
                    println!("created {} (role={role})", created.email);
                    println!("password is EMPTY — anyone knowing the email can log in");
                } else {
                    println!("created {} (role={role})", created.email);
                    println!("password: {}", created.code);
                    println!("store it now — it is not recoverable later");
                }
                Ok(())
            })
        }
        Some("set-role") => {
            let email = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .ok_or_else(|| anyhow::anyhow!("usage: set-role <email> <admin|user>"))?;
            let role = args
                .get(2)
                .map(String::as_str)
                .filter(|r| *r == "admin" || *r == "user")
                .ok_or_else(|| anyhow::anyhow!("usage: set-role <email> <admin|user>"))?;
            account_op(|conn| {
                if users::set_role(conn, email, role)? {
                    println!("{email} is now role={role}");
                } else {
                    eprintln!("no such user: {email}");
                    std::process::exit(1);
                }
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
            let email = args.get(1).ok_or_else(|| {
                anyhow::anyhow!("usage: passwd <email> [--password <new-password>]")
            })?;
            let secret = secret_flag(&args);
            account_op(|conn| match users::reset_code(conn, email, secret)? {
                Some(code) if code.is_empty() => {
                    println!("password for {email} cleared — login now needs only the email");
                    Ok(())
                }
                Some(code) => {
                    println!("new password for {email}: {code}");
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
                "unknown subcommand: {other}\nusage: sync-server [serve | add-user <email> [--password X] [--admin] | set-role <email> <admin|user> | list-users | disable-user <email> | enable-user <email> | passwd <email> [--password X]]"
            );
            std::process::exit(2);
        }
    }
}

/// Value of `--password X` (preferred) or the legacy `--code X`. `None` means
/// the flag is absent (caller generates a random secret); an empty value is
/// meaningful — it creates/clears a passwordless account. A flag at the end of
/// the line or followed by another flag counts as an explicit empty value.
fn secret_flag(args: &[String]) -> Option<String> {
    let pos = args
        .iter()
        .position(|a| a == "--password" || a == "--code")?;
    match args.get(pos + 1) {
        Some(v) if !v.starts_with("--") => Some(v.clone()),
        _ => Some(String::new()),
    }
}

fn parse_add_user_args(args: &[String]) -> anyhow::Result<(String, Option<String>, String)> {
    let email = args
        .get(1)
        .filter(|a| !a.starts_with("--"))
        .ok_or_else(|| anyhow::anyhow!("usage: add-user <email> [--password X] [--admin]"))?;
    let role = if args.iter().any(|a| a == "--admin") {
        "admin"
    } else {
        "user"
    };
    Ok((email.clone(), secret_flag(args), role.to_string()))
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
    for email in users::empty_password_emails(&conn)? {
        tracing::warn!(
            "account {email} has an EMPTY password — anyone knowing the email can log in"
        );
    }
    let state: db::SharedState = Arc::new(db::AppState {
        db: conn.into(),
        cfg,
        login_limiter: Mutex::new(sync_server::ratelimit::LoginLimiter::new()),
        host_snapshot: Mutex::new(None),
        started_at: std::time::Instant::now(),
        backup_busy: std::sync::atomic::AtomicBool::new(false),
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

#[cfg(test)]
mod tests {
    use super::secret_flag;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn secret_flag_parses_password_and_legacy_code() {
        assert_eq!(
            secret_flag(&args(&["add-user", "a@b", "--password", "pw"])),
            Some("pw".into())
        );
        assert_eq!(
            secret_flag(&args(&["add-user", "a@b", "--code", "legacy"])),
            Some("legacy".into())
        );
        // `--password ""` is an explicit passwordless account, not "absent".
        assert_eq!(
            secret_flag(&args(&["add-user", "a@b", "--password", ""])),
            Some("".into())
        );
        // Flag at the end of the line: explicit empty, not random.
        assert_eq!(
            secret_flag(&args(&["passwd", "a@b", "--password"])),
            Some("".into())
        );
        assert_eq!(
            secret_flag(&args(&["passwd", "a@b", "--password", "--json"])),
            Some("".into())
        );
        // No flag at all: caller generates a random secret.
        assert_eq!(secret_flag(&args(&["add-user", "a@b"])), None);
    }
}
