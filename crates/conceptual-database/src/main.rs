use conceptual_database::{http::router, Database, Tree};
use std::{net::SocketAddr, sync::Arc};
use tokio::{net::TcpListener, sync::RwLock};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    const USAGE: &str = "usage: conceptual-database [--listen 127.0.0.1:3000] [--tree std|bbtree]";
    let (mut address, mut tree): (SocketAddr, _) = ("127.0.0.1:3000".parse()?, Tree::Std);
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match (flag.as_str(), args.next().as_deref()) {
            ("--listen", Some(value)) => address = value.parse()?,
            ("--tree", Some("std")) => tree = Tree::Std,
            ("--tree", Some("bbtree")) => tree = Tree::BBTree,
            _ => return Err(USAGE.into()),
        }
    }
    let listener = TcpListener::bind(address).await?;
    println!("Database: http://{}", listener.local_addr()?);
    let database = Arc::new(RwLock::new(Database::with_tree(tree)));
    axum::serve(listener, router(database))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
