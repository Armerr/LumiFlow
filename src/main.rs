mod api;
mod config;
mod embedded;
mod exif;
mod scanner;
mod server;
mod thumbnail;
mod timeline;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("lumiflow=info,tower_http=warn")),
        )
        .init();

    let config = config::Config::from_env()?;
    tracing::info!("photos path: {:?}", config.photos_path);
    tracing::info!("data path: {:?}", config.data_path);

    server::serve(config).await
}
