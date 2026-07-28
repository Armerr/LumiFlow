use rust_embed::RustEmbed;

/// Embedded frontend build output (from `web/dist/`).
/// In development, set `LUMIFLOW_DEV_FRONTEND=1` to serve from disk instead.
#[derive(RustEmbed)]
#[folder = "web/dist/"]
pub struct Frontend;
