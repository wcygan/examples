use two_phase_commit::console::{router, Console};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let port = match args.as_slice() {
        [] => 3000,
        [flag, value] if flag == "--port" => value.parse::<u16>()?,
        _ => return Err("usage: two-phase-commit [--port PORT]".into()),
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("Console: http://{}", listener.local_addr()?);
    topcoat::serve(listener, router(Console::default())).await?;
    Ok(())
}
