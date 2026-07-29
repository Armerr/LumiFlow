use crate::config::AiConfig;
use crate::timeline::db::TimelineDb;
use crate::timeline::models::{AlbumAiDescription, TimelineAlbum};
use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::Utc;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::path::PathBuf;
use std::time::Duration;

const PROMPT_VERSION: &str = "album-description-v1";
const MAX_RESPONSE_BYTES: usize = 1_048_576;
const MAX_DESCRIPTION_CHARS: usize = 1_000;
const MAX_KEYWORDS: usize = 10;
const MAX_KEYWORD_CHARS: usize = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedPhotoSignature {
    pub photo_id: String,
    pub photo_fingerprint: String,
    pub vision_input_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AiDescriptionInput {
    pub album: TimelineAlbum,
    pub time_range: Option<String>,
    pub camera_summary: Vec<String>,
    pub vision_tag_summary: Vec<String>,
    pub selected_photos: Vec<SelectedPhotoSignature>,
    pub contact_sheet_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiDescriptionOutput {
    pub description: String,
    pub keywords: Vec<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub struct ResponsesAiClient {
    client: Client,
    endpoint: Url,
    api_key: String,
    model: String,
    language: String,
}

impl ResponsesAiClient {
    pub fn from_config(config: &AiConfig) -> Result<Self> {
        anyhow::ensure!(config.enabled, "album AI descriptions are disabled");
        let base_url = required_config("AI base URL", config.base_url.as_deref())?;
        let api_key = required_config("AI API key", config.api_key.as_deref())?.to_owned();
        let model = required_config("AI model", config.model.as_deref())?.to_owned();
        let language = required_config("AI language", Some(&config.language))?.to_owned();
        let endpoint = responses_endpoint(base_url)?;
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("failed to build Responses API HTTP client")?;
        Ok(Self {
            client,
            endpoint,
            api_key,
            model,
            language,
        })
    }

    pub fn model_id(&self) -> &str {
        &self.model
    }

    pub async fn generate(&self, input: &AiDescriptionInput) -> Result<AiDescriptionOutput> {
        let jpeg = tokio::fs::read(&input.contact_sheet_path)
            .await
            .with_context(|| {
                format!(
                    "failed to read contact sheet {}",
                    input.contact_sheet_path.display()
                )
            })?;
        anyhow::ensure!(jpeg.starts_with(&[0xff, 0xd8]), "contact sheet is not JPEG");
        let image_url = format!("data:image/jpeg;base64,{}", BASE64_STANDARD.encode(jpeg));
        let request = request_body(&self.model, &self.language, input, image_url);
        let response = self
            .client
            .post(self.endpoint.clone())
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .context("Responses API request failed")?;
        let status = response.status();
        if !status.is_success() {
            bail!("Responses API HTTP {status}");
        }
        let bytes = read_limited_body(response, MAX_RESPONSE_BYTES).await?;
        let response: ResponsesEnvelope =
            serde_json::from_slice(&bytes).context("Responses API returned invalid JSON")?;
        let output_text = extract_output_text(response)?;
        parse_output_text(&output_text)
    }
}

pub fn input_fingerprint(input: &AiDescriptionInput, prompt_version: &str) -> String {
    let mut hasher = Sha1::new();
    fingerprint_string(&mut hasher, prompt_version);
    fingerprint_string(&mut hasher, &input.album.id);
    fingerprint_string(&mut hasher, &input.album.name);
    fingerprint_option(
        &mut hasher,
        input.album.date_start.map(|date| date.to_string()),
    );
    fingerprint_option(
        &mut hasher,
        input.album.date_end.map(|date| date.to_string()),
    );
    fingerprint_option(&mut hasher, input.album.place.clone());
    fingerprint_option(&mut hasher, input.album.holiday.clone());
    fingerprint_string(&mut hasher, &input.album.photo_count.to_string());
    fingerprint_option(&mut hasher, input.album.cover_photo_id.clone());
    fingerprint_option(&mut hasher, input.time_range.clone());
    fingerprint_strings(&mut hasher, &input.camera_summary);
    fingerprint_strings(&mut hasher, &input.vision_tag_summary);
    fingerprint_string(&mut hasher, &input.selected_photos.len().to_string());
    for photo in &input.selected_photos {
        fingerprint_string(&mut hasher, &photo.photo_id);
        fingerprint_string(&mut hasher, &photo.photo_fingerprint);
        fingerprint_option(&mut hasher, photo.vision_input_fingerprint.clone());
    }
    hex_digest(hasher.finalize())
}

pub fn parse_output_text(text: &str) -> Result<AiDescriptionOutput> {
    let output: AiDescriptionOutput =
        serde_json::from_str(text).context("output_text does not match album description shape")?;
    validate_output(&output)?;
    Ok(output)
}

pub async fn generate_or_reuse(
    db: &TimelineDb,
    client: &ResponsesAiClient,
    input: &AiDescriptionInput,
) -> Result<AlbumAiDescription> {
    let fingerprint = input_fingerprint(input, PROMPT_VERSION);
    if let Some(cached) = db
        .get_ai_description(&input.album.id)
        .context("failed to read cached album AI description")?
        .filter(|cached| {
            cached.input_fingerprint == fingerprint
                && cached.model == client.model_id()
                && cached.error.is_none()
        })
    {
        validate_output(&AiDescriptionOutput {
            description: cached.description.clone(),
            keywords: cached.keywords.clone(),
            confidence: cached.confidence,
        })
        .context("cached album AI description is invalid")?;
        return Ok(cached);
    }

    let output = match client.generate(input).await {
        Ok(output) => output,
        Err(error) => {
            db.save_ai_error(&AlbumAiDescription {
                album_id: input.album.id.clone(),
                input_fingerprint: fingerprint.clone(),
                model: client.model_id().to_owned(),
                description: String::new(),
                keywords: Vec::new(),
                confidence: 0.0,
                generated_at: Utc::now().to_rfc3339(),
                error: Some(format!("{error:#}")),
            })
            .context("failed to cache album AI description error")?;
            return Err(error);
        }
    };
    let cached = AlbumAiDescription {
        album_id: input.album.id.clone(),
        input_fingerprint: fingerprint,
        model: client.model_id().to_owned(),
        description: output.description,
        keywords: output.keywords,
        confidence: output.confidence,
        generated_at: Utc::now().to_rfc3339(),
        error: None,
    };
    db.save_ai_description(&cached)
        .context("failed to cache album AI description")?;
    Ok(cached)
}

fn required_config<'a>(name: &str, value: Option<&'a str>) -> Result<&'a str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{name} is required"))
}

fn responses_endpoint(base_url: &str) -> Result<Url> {
    let trimmed = base_url.trim().trim_end_matches('/');
    anyhow::ensure!(!trimmed.is_empty(), "AI base URL is required");
    let endpoint = if trimmed.ends_with("/responses") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/responses")
    };
    Url::parse(&endpoint).with_context(|| format!("invalid AI base URL `{base_url}`"))
}

fn request_body(
    model: &str,
    language: &str,
    input: &AiDescriptionInput,
    image_url: String,
) -> Value {
    json!({
        "model": model,
        "input": [{
            "role": "user",
            "content": [
                {"type": "input_text", "text": prompt(language, input)},
                {"type": "input_image", "image_url": image_url, "detail": "low"}
            ]
        }],
        "text": {"format": {
            "type": "json_schema",
            "name": "album_description",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "description": {"type": "string", "minLength": 1, "maxLength": MAX_DESCRIPTION_CHARS},
                    "keywords": {
                        "type": "array",
                        "items": {"type": "string", "minLength": 1, "maxLength": MAX_KEYWORD_CHARS},
                        "maxItems": MAX_KEYWORDS
                    },
                    "confidence": {"type": "number", "minimum": 0, "maximum": 1}
                },
                "required": ["description", "keywords", "confidence"],
                "additionalProperties": false
            }
        }}
    })
}

fn prompt(language: &str, input: &AiDescriptionInput) -> String {
    format!(
        "Describe this photo album in {language}. Return only the requested structured JSON. Do not create or change a title. Base the description only on the contact sheet and metadata.\nAlbum name: {}\nDate range: {}\nTime range: {}\nPlace: {}\nHoliday: {}\nPhoto count: {}\nCameras: {}\nVision tags: {}",
        input.album.name,
        date_range(input),
        input.time_range.as_deref().unwrap_or("unknown"),
        input.album.place.as_deref().unwrap_or("unknown"),
        input.album.holiday.as_deref().unwrap_or("none"),
        input.album.photo_count,
        summary(&input.camera_summary),
        summary(&input.vision_tag_summary),
    )
}

fn date_range(input: &AiDescriptionInput) -> String {
    match (input.album.date_start, input.album.date_end) {
        (Some(start), Some(end)) if start != end => format!("{start} to {end}"),
        (Some(date), _) | (_, Some(date)) => date.to_string(),
        (None, None) => "unknown".to_owned(),
    }
}

fn summary(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(", ")
    }
}

async fn read_limited_body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        bail!("Responses API response exceeds {limit} bytes");
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0)
            .min(limit),
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .context("failed to read Responses API response")?
    {
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= limit,
            "Responses API response exceeds {limit} bytes"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[derive(Debug, Deserialize)]
struct ResponsesEnvelope {
    #[serde(default)]
    output: Vec<ResponseOutputItem>,
}

#[derive(Debug, Deserialize)]
struct ResponseOutputItem {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content: Vec<ResponseContent>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ResponseContent {
    #[serde(rename = "output_text")]
    OutputText { text: String },
    #[serde(rename = "refusal")]
    Refusal { refusal: String },
    #[serde(other)]
    Other,
}

fn extract_output_text(response: ResponsesEnvelope) -> Result<String> {
    for item in response.output {
        if item.kind != "message" {
            continue;
        }
        for content in item.content {
            match content {
                ResponseContent::OutputText { text } => return Ok(text),
                ResponseContent::Refusal { refusal } => {
                    let _ = refusal;
                    bail!("Responses API refused the album description request");
                }
                ResponseContent::Other => {}
            }
        }
    }
    bail!("Responses API response has no message output_text")
}

fn validate_output(output: &AiDescriptionOutput) -> Result<()> {
    let description_chars = output.description.chars().count();
    anyhow::ensure!(
        (1..=MAX_DESCRIPTION_CHARS).contains(&description_chars)
            && !output.description.trim().is_empty(),
        "description must contain 1 to {MAX_DESCRIPTION_CHARS} characters"
    );
    anyhow::ensure!(
        output.keywords.len() <= MAX_KEYWORDS,
        "keywords must contain at most {MAX_KEYWORDS} entries"
    );
    for keyword in &output.keywords {
        let chars = keyword.chars().count();
        anyhow::ensure!(
            (1..=MAX_KEYWORD_CHARS).contains(&chars) && !keyword.trim().is_empty(),
            "each keyword must contain 1 to {MAX_KEYWORD_CHARS} characters"
        );
    }
    anyhow::ensure!(
        output.confidence.is_finite() && (0.0..=1.0).contains(&output.confidence),
        "confidence must be finite and between 0 and 1"
    );
    Ok(())
}

fn fingerprint_option(hasher: &mut Sha1, value: Option<String>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            fingerprint_string(hasher, &value);
        }
        None => hasher.update([0]),
    }
}

fn fingerprint_strings(hasher: &mut Sha1, values: &[String]) {
    fingerprint_string(hasher, &values.len().to_string());
    for value in values {
        fingerprint_string(hasher, value);
    }
}

fn fingerprint_string(hasher: &mut Sha1, value: &str) {
    hasher.update(value.len().to_be_bytes());
    hasher.update(value.as_bytes());
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(digest.len() * 2);
    for &byte in digest {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use serde_json::{json, Value};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::mpsc;
    use std::thread;

    fn sample_input(path: impl Into<PathBuf>) -> AiDescriptionInput {
        AiDescriptionInput {
            album: TimelineAlbum {
                id: "album-1".into(),
                name: "Spring Walk".into(),
                description: None,
                date_start: NaiveDate::from_ymd_opt(2025, 4, 12),
                date_end: NaiveDate::from_ymd_opt(2025, 4, 12),
                place: Some("Hangzhou".into()),
                holiday: None,
                photo_count: 2,
                cover_photo_id: Some("photo-1".into()),
            },
            time_range: Some("09:00–11:00".into()),
            camera_summary: vec!["Example Camera × 2".into()],
            vision_tag_summary: vec!["garden (1.70)".into(), "people (0.80)".into()],
            selected_photos: vec![
                SelectedPhotoSignature {
                    photo_id: "photo-1".into(),
                    photo_fingerprint: "photo-fp-1".into(),
                    vision_input_fingerprint: Some("vision-fp-1".into()),
                },
                SelectedPhotoSignature {
                    photo_id: "photo-2".into(),
                    photo_fingerprint: "photo-fp-2".into(),
                    vision_input_fingerprint: None,
                },
            ],
            contact_sheet_path: path.into(),
        }
    }

    fn valid_output_json() -> &'static str {
        r#"{"description":"A calm spring walk through a garden.","keywords":["garden","spring"],"confidence":0.86}"#
    }

    #[test]
    fn fingerprint_changes_for_vision_photo_and_prompt_inputs() {
        let original = sample_input("sheet.jpg");
        let base = input_fingerprint(&original, "album-description-v1");

        let mut vision = original.clone();
        vision.vision_tag_summary[0] = "city (1.70)".into();
        assert_ne!(base, input_fingerprint(&vision, "album-description-v1"));

        let mut vision_signature = original.clone();
        vision_signature.selected_photos[0].vision_input_fingerprint = Some("vision-fp-2".into());
        assert_ne!(
            base,
            input_fingerprint(&vision_signature, "album-description-v1")
        );

        let mut photo = original.clone();
        photo.selected_photos[0].photo_fingerprint = "photo-fp-changed".into();
        assert_ne!(base, input_fingerprint(&photo, "album-description-v1"));
        assert_ne!(base, input_fingerprint(&original, "album-description-v2"));
    }

    #[test]
    fn strict_output_parse_accepts_only_valid_bounded_shape() {
        assert_eq!(
            parse_output_text(valid_output_json()).expect("valid output"),
            AiDescriptionOutput {
                description: "A calm spring walk through a garden.".into(),
                keywords: vec!["garden".into(), "spring".into()],
                confidence: 0.86,
            }
        );

        let too_long_description = "x".repeat(1001);
        let too_long_keyword = "k".repeat(65);
        let invalid = [
            json!({"description":"", "keywords":[], "confidence":0.5}),
            json!({"description":too_long_description, "keywords":[], "confidence":0.5}),
            json!({"description":"ok", "keywords":[""], "confidence":0.5}),
            json!({"description":"ok", "keywords":[too_long_keyword], "confidence":0.5}),
            json!({"description":"ok", "keywords":["a","b","c","d","e","f","g","h","i","j","k"], "confidence":0.5}),
            json!({"description":"ok", "keywords":[], "confidence":-0.01}),
            json!({"description":"ok", "keywords":[], "confidence":1.01}),
            json!({"title":"wrong", "description":"ok", "keywords":[], "confidence":0.5}),
            json!({"description":"ok", "keywords":[], "confidence":0.5, "extra":true}),
            json!({"description":"ok", "keywords":"not-an-array", "confidence":0.5}),
        ];
        for value in invalid {
            assert!(
                parse_output_text(&value.to_string()).is_err(),
                "accepted {value}"
            );
        }
        assert!(
            parse_output_text(r#"{"description":"ok","keywords":[],"confidence":1e999}"#).is_err()
        );
    }

    #[tokio::test]
    async fn sends_official_responses_payload_and_extracts_output_text() {
        let dir = TestDir::new("request");
        let sheet = dir.path().join("sheet.jpg");
        std::fs::write(&sheet, [0xff, 0xd8, 0xff, 0xd9]).expect("contact sheet");
        let response = json!({
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type":"output_text", "text":valid_output_json()}]
            }]
        })
        .to_string();
        let server = OneShotServer::start(200, "application/json", response.into_bytes());
        let client = ResponsesAiClient::from_config(&AiConfig {
            enabled: true,
            base_url: Some(format!("{}/v1/", server.base_url())),
            api_key: Some("secret-key".into()),
            model: Some("gpt-test".into()),
            language: "English".into(),
        })
        .expect("client");

        let output = client
            .generate(&sample_input(&sheet))
            .await
            .expect("response");
        assert_eq!(output.confidence, 0.86);
        let request = server.finish();
        assert_eq!(request.path, "/v1/responses");
        assert_eq!(request.authorization.as_deref(), Some("Bearer secret-key"));
        let body: Value = serde_json::from_slice(&request.body).expect("request JSON");
        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert!(body["input"][0]["content"][0]["text"]
            .as_str()
            .expect("prompt")
            .contains("English"));
        assert_eq!(body["input"][0]["content"][1]["type"], "input_image");
        assert_eq!(body["input"][0]["content"][1]["detail"], "low");
        assert_eq!(
            body["input"][0]["content"][1]["image_url"],
            "data:image/jpeg;base64,/9j/2Q=="
        );
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["strict"], true);
        assert_eq!(
            body["text"]["format"]["schema"]["required"],
            json!(["description", "keywords", "confidence"])
        );
        assert_eq!(
            body["text"]["format"]["schema"]["additionalProperties"],
            false
        );
        assert!(body["text"]["format"]["schema"]["properties"]
            .get("title")
            .is_none());
    }

    #[tokio::test]
    async fn base_url_already_ending_in_responses_is_not_appended() {
        let dir = TestDir::new("url");
        let sheet = dir.path().join("sheet.jpg");
        std::fs::write(&sheet, [0xff, 0xd8, 0xff, 0xd9]).expect("contact sheet");
        let response = json!({"output":[{"type":"message","content":[{"type":"output_text","text":valid_output_json()}]}]}).to_string();
        let server = OneShotServer::start(200, "application/json", response.into_bytes());
        let client = ResponsesAiClient::from_config(&AiConfig {
            enabled: true,
            base_url: Some(format!("{}/custom/responses", server.base_url())),
            api_key: Some("key".into()),
            model: Some("model".into()),
            language: "English".into(),
        })
        .expect("client");
        client
            .generate(&sample_input(&sheet))
            .await
            .expect("response");
        assert_eq!(server.finish().path, "/custom/responses");
    }

    #[tokio::test]
    async fn response_errors_cover_http_non_json_refusal_and_missing_output() {
        let dir = TestDir::new("errors");
        let sheet = dir.path().join("sheet.jpg");
        std::fs::write(&sheet, [0xff, 0xd8, 0xff, 0xd9]).expect("contact sheet");
        let cases = [
            (500, "text/plain", b"upstream exploded".to_vec(), "HTTP 500"),
            (200, "application/json", b"not json".to_vec(), "JSON"),
            (
                200,
                "application/json",
                br#"{"output":[{"type":"message","content":[{"type":"refusal","refusal":"unsafe"}]}]}"#.to_vec(),
                "refused",
            ),
            (200, "application/json", br#"{"output":[]}"#.to_vec(), "output_text"),
        ];

        for (status, content_type, response, expected) in cases {
            let server = OneShotServer::start(status, content_type, response);
            let client = ResponsesAiClient::from_config(&AiConfig {
                enabled: true,
                base_url: Some(server.base_url()),
                api_key: Some("key".into()),
                model: Some("model".into()),
                language: "English".into(),
            })
            .expect("client");
            let error = client
                .generate(&sample_input(&sheet))
                .await
                .expect_err("response must fail")
                .to_string();
            assert!(
                error.contains(expected),
                "expected {expected:?} in {error:?}"
            );
            server.finish();
        }
    }

    #[tokio::test]
    async fn generate_or_reuse_caches_valid_result_and_reuses_matching_row() {
        let dir = TestDir::new("cache");
        let sheet = dir.path().join("sheet.jpg");
        std::fs::write(&sheet, [0xff, 0xd8, 0xff, 0xd9]).expect("contact sheet");
        let input = sample_input(&sheet);
        let db = album_db(&input.album);
        let response = json!({"output":[{"type":"message","content":[{"type":"output_text","text":valid_output_json()}]}]}).to_string();
        let server = OneShotServer::start(200, "application/json", response.into_bytes());
        let client = ai_client(server.base_url(), "model-v1");

        let generated = generate_or_reuse(&db, &client, &input)
            .await
            .expect("generate description");
        assert_eq!(generated.album_id, input.album.id);
        assert_eq!(
            generated.description,
            "A calm spring walk through a garden."
        );
        assert_eq!(generated.model, "model-v1");
        assert_eq!(generated.error, None);
        server.finish();
        assert_eq!(
            db.get_ai_description(&input.album.id).expect("cached row"),
            Some(generated.clone())
        );

        let reused = generate_or_reuse(&db, &client, &input)
            .await
            .expect("reuse description without HTTP");
        assert_eq!(reused, generated);
        assert_eq!(db.list_albums().expect("albums")[0].name, "Spring Walk");
    }

    #[tokio::test]
    async fn generation_error_preserves_prior_valid_cached_row() {
        let dir = TestDir::new("cache-error");
        let sheet = dir.path().join("sheet.jpg");
        std::fs::write(&sheet, [0xff, 0xd8, 0xff, 0xd9]).expect("contact sheet");
        let input = sample_input(&sheet);
        let db = album_db(&input.album);
        let prior = AlbumAiDescription {
            album_id: input.album.id.clone(),
            input_fingerprint: "old-fingerprint".into(),
            model: "old-model".into(),
            description: "Prior valid description.".into(),
            keywords: vec!["prior".into()],
            confidence: 0.7,
            generated_at: "2025-01-01T00:00:00Z".into(),
            error: None,
        };
        db.save_ai_description(&prior).expect("save prior cache");
        let server = OneShotServer::start(503, "text/plain", b"unavailable".to_vec());
        let client = ai_client(server.base_url(), "new-model");

        assert!(generate_or_reuse(&db, &client, &input).await.is_err());
        server.finish();
        let after = db
            .get_ai_description(&input.album.id)
            .expect("cached row")
            .expect("prior row preserved");
        assert_eq!(after.description, prior.description);
        assert_eq!(after.keywords, prior.keywords);
        assert_eq!(after.confidence, prior.confidence);
        assert!(after
            .error
            .as_deref()
            .is_some_and(|error| error.contains("HTTP 503")));
    }

    #[tokio::test]
    async fn first_generation_error_is_recorded_for_future_retry() {
        let dir = TestDir::new("first-error");
        let sheet = dir.path().join("sheet.jpg");
        std::fs::write(&sheet, [0xff, 0xd8, 0xff, 0xd9]).expect("contact sheet");
        let input = sample_input(&sheet);
        let db = album_db(&input.album);
        let server = OneShotServer::start(503, "text/plain", b"unavailable".to_vec());
        let client = ai_client(server.base_url(), "new-model");

        assert!(generate_or_reuse(&db, &client, &input).await.is_err());
        server.finish();
        let failed = db
            .get_ai_description(&input.album.id)
            .expect("cached row")
            .expect("error row");
        assert!(failed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("HTTP 503")));
        assert_eq!(failed.model, "new-model");
    }

    fn ai_client(base_url: String, model: &str) -> ResponsesAiClient {
        ResponsesAiClient::from_config(&AiConfig {
            enabled: true,
            base_url: Some(base_url),
            api_key: Some("key".into()),
            model: Some(model.into()),
            language: "English".into(),
        })
        .expect("client")
    }

    fn album_db(album: &TimelineAlbum) -> TimelineDb {
        use crate::timeline::models::{DailyAlbumBuild, PhotoAnalysis, PhotoCandidate, TimeSource};

        let db = TimelineDb::open_in_memory().expect("db");
        for (index, signature) in sample_input("unused").selected_photos.iter().enumerate() {
            db.upsert_candidate(&PhotoCandidate {
                id: signature.photo_id.clone(),
                relative_path: format!("{}.jpg", signature.photo_id),
                filename: format!("{}.jpg", signature.photo_id),
                extension: "jpg".into(),
                size_bytes: 4,
                mtime_ns: index as i64,
                fingerprint: signature.photo_fingerprint.clone(),
                scan_id: "scan".into(),
            })
            .expect("candidate");
            db.save_analysis(&PhotoAnalysis {
                id: signature.photo_id.clone(),
                taken_at: Some(format!("2025-04-12T{:02}:00:00Z", 9 + index)),
                time_source: TimeSource::Exif,
                timezone: Some("UTC".into()),
                gps_lat: None,
                gps_lon: None,
                width: 1,
                height: 1,
                camera_make: None,
                camera_model: None,
                lens: None,
                exif_json: json!({}),
            })
            .expect("analysis");
        }
        db.replace_daily_albums(&[DailyAlbumBuild {
            album: album.clone(),
            photo_ids: vec!["photo-1".into(), "photo-2".into()],
        }])
        .expect("album");
        db
    }

    struct CapturedRequest {
        path: String,
        authorization: Option<String>,
        body: Vec<u8>,
    }

    struct OneShotServer {
        base_url: String,
        request_rx: mpsc::Receiver<CapturedRequest>,
        join: thread::JoinHandle<()>,
    }

    impl OneShotServer {
        fn start(status: u16, content_type: &'static str, response: Vec<u8>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind server");
            let address = listener.local_addr().expect("server address");
            let (request_tx, request_rx) = mpsc::channel();
            let join = thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept request");
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .expect("read timeout");
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 4096];
                let header_end;
                loop {
                    let count = stream.read(&mut chunk).expect("read request");
                    assert!(count > 0, "request ended before headers");
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(index) = find_bytes(&bytes, b"\r\n\r\n") {
                        header_end = index + 4;
                        break;
                    }
                }
                let headers = String::from_utf8(bytes[..header_end].to_vec()).expect("headers");
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("content length"))
                        })
                    })
                    .expect("content-length header");
                while bytes.len() < header_end + content_length {
                    let count = stream.read(&mut chunk).expect("read body");
                    assert!(count > 0, "request ended before body");
                    bytes.extend_from_slice(&chunk[..count]);
                }
                let request_line = headers.lines().next().expect("request line");
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .expect("request path")
                    .to_owned();
                let authorization = headers.lines().find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("authorization")
                            .then(|| value.trim().to_owned())
                    })
                });
                request_tx
                    .send(CapturedRequest {
                        path,
                        authorization,
                        body: bytes[header_end..header_end + content_length].to_vec(),
                    })
                    .expect("capture request");
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .expect("write response headers");
                stream.write_all(&response).expect("write response body");
            });
            Self {
                base_url: format!("http://{address}"),
                request_rx,
                join,
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }

        fn finish(self) -> CapturedRequest {
            let request = self
                .request_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("captured request");
            self.join.join().expect("server thread");
            request
        }
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "lumiflow-ai-{label}-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
