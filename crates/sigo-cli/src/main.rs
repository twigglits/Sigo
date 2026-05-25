fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    println!("sigo v0.1.0 — not wired up yet");
    Ok(())
}
