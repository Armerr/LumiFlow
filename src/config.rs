use anyhow::{bail, Context};
use serde::Serialize;
use std::env;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum AlbumMode {
    Folders,
    Timeline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum VisionTagger {
    None,
    OnnxMobileClip,
    OpenVinoMobileClip,
}

#[derive(Clone, Debug, Serialize)]
pub struct AiConfig {
    pub enabled: bool,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub language: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Config {
    pub photos_path: PathBuf,
    pub data_path: PathBuf,
    pub bind_address: String,
    pub port: u16,
    pub builder_workers: usize,
    pub exclude_regex: String,
    pub album_mode: AlbumMode,
    pub timeline_timezone: String,
    pub calendar_region: String,
    pub place_provider: Option<String>,
    pub place_base_url: Option<String>,
    pub vision_tagger: VisionTagger,
    pub vision_model_path: Option<PathBuf>,
    pub vision_labels_path: Option<PathBuf>,
    pub vision_workers: usize,
    pub ai: AiConfig,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            photos_path: required_path("LUMIFLOW_PHOTOS_PATH")?,
            data_path: required_path("LUMIFLOW_DATA_PATH")?,
            bind_address: env::var("LUMIFLOW_BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0".into()),
            port: parse_or("LUMIFLOW_PORT", 4320)?,
            builder_workers: parse_or("LUMIFLOW_BUILDER_WORKERS", 2)?,
            exclude_regex: env::var("LUMIFLOW_EXCLUDE_REGEX")
                .unwrap_or_else(|_| r"(^|/)(@eaDir|#recycle|\.DS_Store|Thumbs\.db)(/|$)".into()),
            album_mode: parse_album_mode()?,
            timeline_timezone: non_empty_or("LUMIFLOW_TIMELINE_TIMEZONE", "Asia/Shanghai")?,
            calendar_region: non_empty_or("LUMIFLOW_CALENDAR_REGION", "CN_COMMON")?,
            place_provider: parse_place_provider()?,
            place_base_url: optional_string("LUMIFLOW_PLACE_BASE_URL"),
            vision_tagger: parse_vision_tagger()?,
            vision_model_path: optional_path("LUMIFLOW_VISION_MODEL_PATH"),
            vision_labels_path: optional_path("LUMIFLOW_VISION_LABELS_PATH"),
            vision_workers: parse_positive_or("LUMIFLOW_VISION_WORKERS", 1)?,
            ai: parse_ai_config()?,
        })
    }
}

fn required_path(key: &str) -> anyhow::Result<PathBuf> {
    let value = env::var(key).with_context(|| format!("{key} must be set"))?;
    if value.trim().is_empty() {
        bail!("{key} must not be empty");
    }
    Ok(PathBuf::from(value))
}

fn optional_path(key: &str) -> Option<PathBuf> {
    optional_string(key).map(PathBuf::from)
}

fn optional_string(key: &str) -> Option<String> {
    env::var(key).ok().and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn non_empty_or(key: &str, default: &str) -> anyhow::Result<String> {
    match env::var(key) {
        Ok(value) if value.trim().is_empty() => bail!("{key} must not be empty"),
        Ok(value) => Ok(value),
        Err(env::VarError::NotPresent) => Ok(default.to_owned()),
        Err(error) => Err(error).with_context(|| format!("failed to read {key}")),
    }
}

fn parse_or<T>(key: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match env::var(key) {
        Ok(value) => value
            .parse()
            .with_context(|| format!("invalid {key} value `{value}`")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error).with_context(|| format!("failed to read {key}")),
    }
}

fn parse_positive_or(key: &str, default: usize) -> anyhow::Result<usize> {
    let value = parse_or(key, default)?;
    if value == 0 {
        bail!("{key} must be at least 1");
    }
    Ok(value)
}

fn parse_album_mode() -> anyhow::Result<AlbumMode> {
    match env::var("LUMIFLOW_ALBUM_MODE") {
        Ok(value) => match value.as_str() {
            "folders" => Ok(AlbumMode::Folders),
            "timeline" => Ok(AlbumMode::Timeline),
            _ => {
                bail!("unsupported LUMIFLOW_ALBUM_MODE `{value}`; expected `folders` or `timeline`")
            }
        },
        Err(env::VarError::NotPresent) => Ok(AlbumMode::Folders),
        Err(error) => Err(error).context("failed to read LUMIFLOW_ALBUM_MODE"),
    }
}

fn parse_vision_tagger() -> anyhow::Result<VisionTagger> {
    match env::var("LUMIFLOW_VISION_TAGGER") {
        Ok(value) => match value.as_str() {
            "none" => Ok(VisionTagger::None),
            "onnx-mobileclip" => Ok(VisionTagger::OnnxMobileClip),
            "openvino-mobileclip" => Ok(VisionTagger::OpenVinoMobileClip),
            _ => bail!(
                "unsupported LUMIFLOW_VISION_TAGGER `{value}`; expected `none`, `onnx-mobileclip`, or `openvino-mobileclip`"
            ),
        },
        Err(env::VarError::NotPresent) => Ok(VisionTagger::None),
        Err(error) => Err(error).context("failed to read LUMIFLOW_VISION_TAGGER"),
    }
}

fn parse_optional_provider(key: &str, supported: &[&str]) -> anyhow::Result<Option<String>> {
    let Some(value) = optional_string(key) else {
        return Ok(None);
    };
    if supported.contains(&value.as_str()) {
        Ok(Some(value))
    } else {
        bail!(
            "unsupported {key} `{value}`; expected one of: {}",
            supported.join(", ")
        )
    }
}

fn parse_place_provider() -> anyhow::Result<Option<String>> {
    let provider = parse_optional_provider("LUMIFLOW_PLACE_PROVIDER", &["nominatim"])?;
    if provider.as_deref() == Some("nominatim")
        && optional_string("LUMIFLOW_PLACE_BASE_URL").is_none()
    {
        bail!(
            "LUMIFLOW_PLACE_BASE_URL must be set and non-empty when LUMIFLOW_PLACE_PROVIDER=nominatim"
        );
    }
    Ok(provider)
}

fn parse_ai_config() -> anyhow::Result<AiConfig> {
    let enabled = parse_or("LUMIFLOW_AI_ENABLED", false)?;
    let provider = optional_string("LUMIFLOW_AI_PROVIDER");
    if let Some(provider) = provider.as_deref() {
        if provider != "openai-compatible" {
            bail!("unsupported LUMIFLOW_AI_PROVIDER `{provider}`; expected `openai-compatible`");
        }
    }

    let base_url = optional_string("LUMIFLOW_AI_BASE_URL");
    let api_key = optional_string("LUMIFLOW_AI_API_KEY");
    let model = optional_string("LUMIFLOW_AI_MODEL");
    if enabled {
        for (key, value) in [
            ("LUMIFLOW_AI_BASE_URL", base_url.as_ref()),
            ("LUMIFLOW_AI_API_KEY", api_key.as_ref()),
            ("LUMIFLOW_AI_MODEL", model.as_ref()),
        ] {
            if value.is_none() {
                bail!("{key} must be set and non-empty when LUMIFLOW_AI_ENABLED=true");
            }
        }
    }

    Ok(AiConfig {
        enabled,
        base_url,
        api_key,
        model,
        language: non_empty_or("LUMIFLOW_AI_DESCRIPTION_LANGUAGE", "zh-CN")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ENRICHMENT_ENV: &[&str] = &[
        "LUMIFLOW_PHOTOS_PATH",
        "LUMIFLOW_DATA_PATH",
        "LUMIFLOW_PORT",
        "LUMIFLOW_ALBUM_MODE",
        "LUMIFLOW_TIMELINE_TIMEZONE",
        "LUMIFLOW_CALENDAR_REGION",
        "LUMIFLOW_PLACE_PROVIDER",
        "LUMIFLOW_PLACE_BASE_URL",
        "LUMIFLOW_VISION_TAGGER",
        "LUMIFLOW_VISION_MODEL_PATH",
        "LUMIFLOW_VISION_LABELS_PATH",
        "LUMIFLOW_VISION_WORKERS",
        "LUMIFLOW_AI_ENABLED",
        "LUMIFLOW_AI_PROVIDER",
        "LUMIFLOW_AI_BASE_URL",
        "LUMIFLOW_AI_API_KEY",
        "LUMIFLOW_AI_MODEL",
        "LUMIFLOW_AI_DESCRIPTION_LANGUAGE",
    ];

    struct EnvSnapshot(Vec<(&'static str, Option<OsString>)>);

    impl EnvSnapshot {
        fn capture(keys: &'static [&'static str]) -> Self {
            Self(keys.iter().map(|&key| (key, env::var_os(key))).collect())
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
        }
    }

    fn clear_env(keys: &[&str]) {
        for key in keys {
            env::remove_var(key);
        }
    }

    fn set_required_paths() {
        env::set_var("LUMIFLOW_PHOTOS_PATH", "/tmp/lumiflow-photos");
        env::set_var("LUMIFLOW_DATA_PATH", "/tmp/lumiflow-data");
    }

    #[test]
    fn config_defaults_to_port_4320() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _snapshot = EnvSnapshot::capture(&[
            "LUMIFLOW_PHOTOS_PATH",
            "LUMIFLOW_DATA_PATH",
            "LUMIFLOW_PORT",
        ]);
        set_required_paths();
        env::remove_var("LUMIFLOW_PORT");

        let config = Config::from_env().expect("config");
        assert_eq!(config.port, 4320);
    }

    #[test]
    fn config_defaults_to_folder_mode_with_optional_enrichment_disabled() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _snapshot = EnvSnapshot::capture(ENRICHMENT_ENV);
        clear_env(ENRICHMENT_ENV);
        set_required_paths();

        let config = Config::from_env().expect("config");
        assert_eq!(config.port, 4320);
        assert_eq!(config.album_mode, AlbumMode::Folders);
        assert_eq!(config.timeline_timezone, "Asia/Shanghai");
        assert_eq!(config.calendar_region, "CN_COMMON");
        assert_eq!(config.place_provider, None);
        assert_eq!(config.place_base_url, None);
        assert_eq!(config.vision_tagger, VisionTagger::None);
        assert_eq!(config.vision_model_path, None);
        assert_eq!(config.vision_labels_path, None);
        assert_eq!(config.vision_workers, 1);
        assert!(!config.ai.enabled);
        assert_eq!(config.ai.base_url, None);
        assert_eq!(config.ai.api_key, None);
        assert_eq!(config.ai.model, None);
        assert_eq!(config.ai.language, "zh-CN");
    }

    #[test]
    fn config_parses_timeline_and_ai_settings() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _snapshot = EnvSnapshot::capture(ENRICHMENT_ENV);
        clear_env(ENRICHMENT_ENV);
        set_required_paths();
        env::set_var("LUMIFLOW_ALBUM_MODE", "timeline");
        env::set_var("LUMIFLOW_TIMELINE_TIMEZONE", "Europe/Paris");
        env::set_var("LUMIFLOW_CALENDAR_REGION", "FR_COMMON");
        env::set_var("LUMIFLOW_PLACE_PROVIDER", "nominatim");
        env::set_var("LUMIFLOW_PLACE_BASE_URL", "https://nominatim.example.test");
        env::set_var("LUMIFLOW_VISION_TAGGER", "onnx-mobileclip");
        env::set_var("LUMIFLOW_VISION_MODEL_PATH", "/models/mobileclip.onnx");
        env::set_var("LUMIFLOW_VISION_LABELS_PATH", "/models/labels.json");
        env::set_var("LUMIFLOW_VISION_WORKERS", "3");
        env::set_var("LUMIFLOW_AI_ENABLED", "true");
        env::set_var("LUMIFLOW_AI_PROVIDER", "openai-compatible");
        env::set_var("LUMIFLOW_AI_BASE_URL", "https://example.invalid/v1");
        env::set_var("LUMIFLOW_AI_API_KEY", "test-key");
        env::set_var("LUMIFLOW_AI_MODEL", "vision-model");
        env::set_var("LUMIFLOW_AI_DESCRIPTION_LANGUAGE", "en-US");

        let config = Config::from_env().expect("config");
        assert_eq!(config.album_mode, AlbumMode::Timeline);
        assert_eq!(config.timeline_timezone, "Europe/Paris");
        assert_eq!(config.calendar_region, "FR_COMMON");
        assert_eq!(config.place_provider.as_deref(), Some("nominatim"));
        assert_eq!(
            config.place_base_url.as_deref(),
            Some("https://nominatim.example.test")
        );
        assert_eq!(config.vision_tagger, VisionTagger::OnnxMobileClip);
        assert_eq!(
            config.vision_model_path.as_deref(),
            Some(std::path::Path::new("/models/mobileclip.onnx"))
        );
        assert_eq!(
            config.vision_labels_path.as_deref(),
            Some(std::path::Path::new("/models/labels.json"))
        );
        assert_eq!(config.vision_workers, 3);
        assert!(config.ai.enabled);
        assert_eq!(
            config.ai.base_url.as_deref(),
            Some("https://example.invalid/v1")
        );
        assert_eq!(config.ai.api_key.as_deref(), Some("test-key"));
        assert_eq!(config.ai.model.as_deref(), Some("vision-model"));
        assert_eq!(config.ai.language, "en-US");
    }
    #[test]
    fn nominatim_provider_requires_non_empty_base_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _snapshot = EnvSnapshot::capture(ENRICHMENT_ENV);
        clear_env(ENRICHMENT_ENV);
        set_required_paths();
        env::set_var("LUMIFLOW_PLACE_PROVIDER", "nominatim");

        let error = Config::from_env().expect_err("missing place base URL must fail");
        assert!(error
            .to_string()
            .contains("LUMIFLOW_PLACE_BASE_URL must be set and non-empty when LUMIFLOW_PLACE_PROVIDER=nominatim"));

        env::set_var("LUMIFLOW_PLACE_BASE_URL", "   ");
        let error = Config::from_env().expect_err("blank place base URL must fail");
        assert!(error
            .to_string()
            .contains("LUMIFLOW_PLACE_BASE_URL must be set and non-empty when LUMIFLOW_PLACE_PROVIDER=nominatim"));
    }

    #[test]
    fn disabled_place_provider_keeps_base_url_without_enabling_network() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _snapshot = EnvSnapshot::capture(ENRICHMENT_ENV);
        clear_env(ENRICHMENT_ENV);
        set_required_paths();
        env::set_var("LUMIFLOW_PLACE_BASE_URL", "https://nominatim.example.test");

        let config = Config::from_env().expect("disabled place config");
        assert_eq!(config.place_provider, None);
        assert_eq!(
            config.place_base_url.as_deref(),
            Some("https://nominatim.example.test")
        );
    }

    #[test]
    fn enabled_ai_requires_non_empty_credentials() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _snapshot = EnvSnapshot::capture(ENRICHMENT_ENV);
        clear_env(ENRICHMENT_ENV);
        set_required_paths();
        env::set_var("LUMIFLOW_AI_ENABLED", "true");
        env::set_var("LUMIFLOW_AI_API_KEY", "test-key");
        env::set_var("LUMIFLOW_AI_MODEL", "vision-model");

        let error = Config::from_env().expect_err("missing AI base URL must fail");
        assert!(error
            .to_string()
            .contains("LUMIFLOW_AI_BASE_URL must be set and non-empty"));
    }

    #[test]
    fn config_rejects_unsupported_provider_names() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _snapshot = EnvSnapshot::capture(ENRICHMENT_ENV);
        clear_env(ENRICHMENT_ENV);
        set_required_paths();
        env::set_var("LUMIFLOW_VISION_TAGGER", "mobileclip_onnx");

        let error = Config::from_env().expect_err("invalid vision provider must fail");
        let message = error.to_string();
        assert!(message.contains("unsupported LUMIFLOW_VISION_TAGGER `mobileclip_onnx`"));
        assert!(message.contains("`onnx-mobileclip`"));
    }
}
