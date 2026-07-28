use serde::Serialize;
use std::env;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize)]
pub struct Config {
    pub photos_path: PathBuf,
    pub data_path: PathBuf,
    pub bind_address: String,
    pub port: u16,
    pub builder_workers: usize,
    pub exclude_regex: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            photos_path: env_var("LUMIFLOW_PHOTOS_PATH")?,
            data_path: env_var("LUMIFLOW_DATA_PATH")?,
            bind_address: env::var("LUMIFLOW_BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("LUMIFLOW_PORT")
                .unwrap_or_else(|_| "4320".into())
                .parse()?,
            builder_workers: env::var("LUMIFLOW_BUILDER_WORKERS")
                .unwrap_or_else(|_| "2".into())
                .parse()?,
            exclude_regex: env::var("LUMIFLOW_EXCLUDE_REGEX")
                .unwrap_or_else(|_| r"(^|/)(@eaDir|#recycle|\.DS_Store|Thumbs\.db)(/|$)".into()),
        })
    }
}

fn env_var(key: &str) -> anyhow::Result<PathBuf> {
    let val = env::var(key)?;
    Ok(PathBuf::from(&val))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn config_defaults_to_port_4320() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let old_photos = env::var("LUMIFLOW_PHOTOS_PATH").ok();
        let old_data = env::var("LUMIFLOW_DATA_PATH").ok();
        let old_port = env::var("LUMIFLOW_PORT").ok();

        env::set_var("LUMIFLOW_PHOTOS_PATH", "/tmp/lumiflow-photos");
        env::set_var("LUMIFLOW_DATA_PATH", "/tmp/lumiflow-data");
        env::remove_var("LUMIFLOW_PORT");

        let config = Config::from_env().expect("config");
        assert_eq!(config.port, 4320);

        restore_env("LUMIFLOW_PHOTOS_PATH", old_photos);
        restore_env("LUMIFLOW_DATA_PATH", old_data);
        restore_env("LUMIFLOW_PORT", old_port);
    }

    fn restore_env(key: &str, value: Option<String>) {
        match value {
            Some(value) => env::set_var(key, value),
            None => env::remove_var(key),
        }
    }
}
