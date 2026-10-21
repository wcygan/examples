use conceptual_database::{http::router, Database};
use std::{net::SocketAddr, sync::Arc};
use tokio::{net::TcpListener, sync::RwLock};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let address: SocketAddr = match (args.next().as_deref(), args.next(), args.next()) {
        (None, None, None) => "127.0.0.1:3000".parse()?,
        (Some("--listen"), Some(address), None) => address.parse()?,
        _ => return Err("usage: conceptual-database [--listen 127.0.0.1:3000]".into()),
    };
    let listener = TcpListener::bind(address).await?;
    println!("Database: http://{}", listener.local_addr()?);
    let database = Arc::new(RwLock::new(Database::default()));
    axum::serve(listener, router(database))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
