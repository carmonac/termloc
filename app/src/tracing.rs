use tracing::subscriber;

/// Installs a no-op global subscriber so `tracing` spans and events don't produce any output.
pub fn init() -> anyhow::Result<()> {
    subscriber::set_global_default(subscriber::NoSubscriber::new())?;
    Ok(())
}
