use crate::timeline::db::TimelineDb;
use crate::timeline::models::VisionTags;
use anyhow::{bail, Context, Result};
use chrono::Utc;
use sha1::{Digest, Sha1};
use std::path::Path;

const MAX_TAGS: usize = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct VisionTag {
    pub label: String,
    pub score: f32,
}

pub trait VisionTagger {
    fn model_id(&self) -> &str;
    fn tag(&mut self, thumbnail_path: &Path) -> Result<Vec<VisionTag>>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum TagPhotoOutcome {
    Disabled,
    Cached(Vec<VisionTag>),
    Tagged(Vec<VisionTag>),
}

pub fn tag_photo<T: VisionTagger + ?Sized>(
    db: &TimelineDb,
    tagger: &mut T,
    photo_id: &str,
    photo_fingerprint: &str,
    thumbnail_path: &Path,
    thumbnail_fingerprint: &str,
    tagset_version: &str,
) -> Result<TagPhotoOutcome> {
    validate_component("photo ID", photo_id)?;
    validate_component("photo fingerprint", photo_fingerprint)?;
    validate_component("thumbnail fingerprint", thumbnail_fingerprint)?;
    validate_component("model ID", tagger.model_id())?;
    validate_component("tagset version", tagset_version)?;

    let model_id = tagger.model_id().to_owned();
    let input_fingerprint = cache_fingerprint(
        photo_fingerprint,
        thumbnail_fingerprint,
        &model_id,
        tagset_version,
    );

    if let Some(cached) = db
        .get_vision_tags(photo_id, &model_id)
        .context("failed to read cached vision tags")?
        .filter(|cached| cached.input_fingerprint == input_fingerprint && cached.error.is_none())
    {
        let tags = tags_from_cache(cached)?;
        validate_tags(&tags).context("cached vision tags are invalid")?;
        return Ok(TagPhotoOutcome::Cached(tags));
    }

    let tags = match tagger.tag(thumbnail_path) {
        Ok(tags) => normalize_tags(tags)?,
        Err(error) => {
            let error = anyhow::anyhow!("vision tagger failed: {error:#}");
            db.save_vision_error(&VisionTags {
                photo_id: photo_id.to_owned(),
                model: model_id.clone(),
                input_fingerprint: input_fingerprint.clone(),
                labels: Vec::new(),
                scores: Vec::new(),
                analyzed_at: Utc::now().to_rfc3339(),
                error: Some(format!("{error:#}")),
            })
            .context("failed to cache vision tagging error")?;
            return Err(error);
        }
    };
    let cached = VisionTags {
        photo_id: photo_id.to_owned(),
        model: model_id,
        input_fingerprint,
        labels: tags.iter().map(|tag| tag.label.clone()).collect(),
        scores: tags.iter().map(|tag| tag.score).collect(),
        analyzed_at: Utc::now().to_rfc3339(),
        error: None,
    };
    db.save_vision_tags(&cached)
        .context("failed to cache vision tags")?;

    Ok(TagPhotoOutcome::Tagged(tags))
}

pub fn cache_fingerprint(
    photo_fingerprint: &str,
    thumbnail_fingerprint: &str,
    model_id: &str,
    tagset_version: &str,
) -> String {
    let mut hasher = Sha1::new();
    for component in [
        photo_fingerprint,
        thumbnail_fingerprint,
        model_id,
        tagset_version,
    ] {
        hasher.update(component.len().to_be_bytes());
        hasher.update(component.as_bytes());
    }
    hex_digest(hasher.finalize())
}

fn validate_component(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{name} must not be empty");
    }
    Ok(())
}

fn normalize_tags(mut tags: Vec<VisionTag>) -> Result<Vec<VisionTag>> {
    validate_tags(&tags)?;
    tags.sort_by(|left, right| right.score.total_cmp(&left.score));
    tags.truncate(MAX_TAGS);
    Ok(tags)
}

fn validate_tags(tags: &[VisionTag]) -> Result<()> {
    for tag in tags {
        if tag.label.trim().is_empty() {
            bail!("vision tag label must not be empty");
        }
        if !tag.score.is_finite() || !(0.0..=1.0).contains(&tag.score) {
            bail!(
                "vision tag score for `{}` must be finite and between 0 and 1",
                tag.label
            );
        }
    }
    Ok(())
}

fn tags_from_cache(cached: VisionTags) -> Result<Vec<VisionTag>> {
    if cached.labels.len() != cached.scores.len() {
        bail!(
            "cached vision tags have {} labels but {} scores",
            cached.labels.len(),
            cached.scores.len()
        );
    }
    Ok(cached
        .labels
        .into_iter()
        .zip(cached.scores)
        .map(|(label, score)| VisionTag { label, score })
        .collect())
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
#[derive(Debug, Default, Clone, Copy)]
pub struct DisabledTagger;

impl VisionTagger for DisabledTagger {
    fn model_id(&self) -> &str {
        "disabled"
    }

    fn tag(&mut self, _thumbnail_path: &Path) -> Result<Vec<VisionTag>> {
        bail!("vision tagging is disabled")
    }
}

pub fn tag_photo_disabled() -> TagPhotoOutcome {
    TagPhotoOutcome::Disabled
}

#[cfg(feature = "vision-onnx")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LabelFile {
    version: u32,
    model_id: String,
    tagset_version: String,
    image_size: usize,
    mean: [f32; 3],
    std: [f32; 3],
    labels: Vec<LabelEmbedding>,
}

#[cfg(feature = "vision-onnx")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LabelEmbedding {
    label: String,
    embedding: Vec<f32>,
}

#[cfg(feature = "vision-onnx")]
#[derive(Debug)]
pub struct OnnxMobileClipTagger {
    session: ort::session::Session,
    model_id: String,
    tagset_version: String,
    image_size: usize,
    mean: [f32; 3],
    std: [f32; 3],
    labels: Vec<LabelEmbedding>,
    embedding_dim: usize,
}

#[cfg(feature = "vision-onnx")]
impl OnnxMobileClipTagger {
    pub fn load(model_path: &Path, labels_path: &Path) -> Result<Self> {
        Self::load_with_threads(model_path, labels_path, 1)
    }

    pub fn load_with_threads(
        model_path: &Path,
        labels_path: &Path,
        intra_threads: usize,
    ) -> Result<Self> {
        if intra_threads == 0 {
            bail!("ONNX intra-op threads must be positive");
        }
        require_explicit_asset("model", model_path)?;
        require_explicit_asset("labels", labels_path)?;

        let labels_json = std::fs::read(labels_path).with_context(|| {
            format!(
                "failed to read explicit vision labels asset `{}`",
                labels_path.display()
            )
        })?;
        let mut label_file: LabelFile =
            serde_json::from_slice(&labels_json).with_context(|| {
                format!(
                    "failed to parse vision labels JSON `{}`",
                    labels_path.display()
                )
            })?;
        validate_label_file(&mut label_file)?;

        let builder =
            ort::session::Session::builder().context("failed to create ONNX session builder")?;
        let mut builder = builder.with_intra_threads(intra_threads).map_err(|error| {
            anyhow::anyhow!("failed to configure ONNX intra-op threads: {error}")
        })?;
        let session = builder
            .commit_from_file(model_path)
            .with_context(|| format!("failed to load ONNX model `{}`", model_path.display()))?;
        let embedding_dim = validate_session_contract(&session, label_file.image_size)?;
        let label_embedding_dim = label_file.labels[0].embedding.len();
        if label_embedding_dim != embedding_dim {
            bail!(
                "vision labels embedding dimension {label_embedding_dim} does not match ONNX output dimension {embedding_dim}"
            );
        }

        Ok(Self {
            session,
            model_id: label_file.model_id,
            tagset_version: label_file.tagset_version,
            image_size: label_file.image_size,
            mean: label_file.mean,
            std: label_file.std,
            labels: label_file.labels,
            embedding_dim,
        })
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn tagset_version(&self) -> &str {
        &self.tagset_version
    }
}

#[cfg(feature = "vision-onnx")]
impl VisionTagger for OnnxMobileClipTagger {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn tag(&mut self, thumbnail_path: &Path) -> Result<Vec<VisionTag>> {
        let image = image::open(thumbnail_path)
            .with_context(|| format!("failed to decode thumbnail `{}`", thumbnail_path.display()))?
            .to_rgb8();
        let input = preprocess_rgb_image(&image, self.image_size, self.mean, self.std)
            .context("failed to preprocess thumbnail for MobileCLIP")?;
        let tensor =
            ort::value::Tensor::from_array(([1usize, 3, self.image_size, self.image_size], input))
                .context("failed to create MobileCLIP input tensor")?;
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .context("MobileCLIP ONNX inference failed")?;
        let (shape, values) = outputs[0]
            .try_extract_tensor::<f32>()
            .context("failed to extract MobileCLIP float32 output tensor")?;
        validate_runtime_output_shape(shape, self.embedding_dim)?;
        let image_embedding = normalized_copy(values, "MobileCLIP image embedding")?;

        probabilities_for_labels(&self.labels, &image_embedding)
    }
}

#[cfg(feature = "vision-onnx")]
fn validate_label_file(label_file: &mut LabelFile) -> Result<()> {
    if label_file.version != 1 {
        bail!(
            "unsupported vision labels schema version {}; expected 1",
            label_file.version
        );
    }
    validate_nonempty("model ID", &label_file.model_id)?;
    validate_nonempty("tagset version", &label_file.tagset_version)?;
    if label_file.image_size == 0 {
        bail!("vision labels image_size must be a positive image_size");
    }
    for (channel, value) in label_file.mean.iter().copied().enumerate() {
        if !value.is_finite() {
            bail!("vision labels preprocessing requires finite mean values; channel {channel} is {value}");
        }
    }
    for (channel, value) in label_file.std.iter().copied().enumerate() {
        if !value.is_finite() || value == 0.0 {
            bail!("vision labels preprocessing requires finite nonzero std values; channel {channel} is {value}");
        }
    }
    if label_file.labels.is_empty() {
        bail!("vision labels must contain at least one label");
    }

    let embedding_dim = label_file.labels[0].embedding.len();
    if embedding_dim == 0 {
        bail!("vision label embedding dimension must be positive");
    }
    let mut seen = std::collections::HashSet::with_capacity(label_file.labels.len());
    for entry in &mut label_file.labels {
        validate_nonempty("label", &entry.label)?;
        if !seen.insert(entry.label.clone()) {
            bail!("duplicate label `{}` in vision labels", entry.label);
        }
        if entry.embedding.len() != embedding_dim {
            bail!(
                "vision label `{}` has embedding dimension {}, expected {embedding_dim}",
                entry.label,
                entry.embedding.len()
            );
        }
        normalize_in_place(
            &mut entry.embedding,
            &format!("vision label `{}` embedding", entry.label),
        )?;
    }
    Ok(())
}

#[cfg(feature = "vision-onnx")]
fn validate_nonempty(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("vision labels {name} must not be empty");
    }
    Ok(())
}

#[cfg(feature = "vision-onnx")]
fn validate_session_contract(session: &ort::session::Session, image_size: usize) -> Result<usize> {
    use ort::value::TensorElementType;

    if session.inputs().len() != 1 {
        bail!(
            "MobileCLIP ONNX contract requires exactly one input, model has {}",
            session.inputs().len()
        );
    }
    if session.outputs().len() != 1 {
        bail!(
            "MobileCLIP ONNX contract requires exactly one output, model has {}",
            session.outputs().len()
        );
    }

    let input_type = session.inputs()[0].dtype();
    if input_type.tensor_type() != Some(TensorElementType::Float32) {
        bail!("MobileCLIP ONNX input must be float32, found {input_type}");
    }
    let input_shape = input_type
        .tensor_shape()
        .context("MobileCLIP ONNX input must be a tensor")?;
    let expected_size = i64::try_from(image_size).context("vision image_size is too large")?;
    if input_shape.as_ref() != [1, 3, expected_size, expected_size] {
        bail!(
            "MobileCLIP ONNX input must have fixed NCHW shape [1, 3, {image_size}, {image_size}] matching JSON image_size {image_size}; found {input_shape}"
        );
    }

    let output_type = session.outputs()[0].dtype();
    if output_type.tensor_type() != Some(TensorElementType::Float32) {
        bail!("MobileCLIP ONNX output must be float32, found {output_type}");
    }
    let output_shape = output_type
        .tensor_shape()
        .context("MobileCLIP ONNX output must be a tensor")?;
    let output_dim = match output_shape.as_ref() {
        [dimension] if *dimension > 0 => *dimension,
        [1, dimension] if *dimension > 0 => *dimension,
        _ => bail!(
            "MobileCLIP ONNX output must have fixed float32 shape [D] or [1, D] with positive D; found {output_shape}"
        ),
    };
    usize::try_from(output_dim).context("MobileCLIP ONNX output dimension is too large")
}

#[cfg(feature = "vision-onnx")]
fn validate_runtime_output_shape(shape: &ort::value::Shape, embedding_dim: usize) -> Result<()> {
    let embedding_dim = i64::try_from(embedding_dim).context("embedding dimension is too large")?;
    if shape.as_ref() != [embedding_dim] && shape.as_ref() != [1, embedding_dim] {
        bail!(
            "MobileCLIP inference returned output shape {shape}; expected [{embedding_dim}] or [1, {embedding_dim}]"
        );
    }
    Ok(())
}

#[cfg(feature = "vision-onnx")]
fn normalize_in_place(values: &mut [f32], description: &str) -> Result<()> {
    let mut squared_norm = 0.0_f64;
    for (index, value) in values.iter().copied().enumerate() {
        if !value.is_finite() {
            bail!("{description} must contain only finite values; index {index} is {value}");
        }
        squared_norm += f64::from(value) * f64::from(value);
    }
    if !squared_norm.is_finite() || squared_norm == 0.0 {
        bail!("{description} must be finite and nonzero");
    }
    let inverse_norm = (1.0 / squared_norm.sqrt()) as f32;
    for value in values {
        *value *= inverse_norm;
    }
    Ok(())
}

#[cfg(feature = "vision-onnx")]
fn normalized_copy(values: &[f32], description: &str) -> Result<Vec<f32>> {
    let mut normalized = values.to_vec();
    normalize_in_place(&mut normalized, description)?;
    Ok(normalized)
}

#[cfg(feature = "vision-onnx")]
fn preprocess_rgb_image(
    image: &image::RgbImage,
    image_size: usize,
    mean: [f32; 3],
    std: [f32; 3],
) -> Result<Vec<f32>> {
    if image.width() == 0 || image.height() == 0 {
        bail!("thumbnail has zero width or height");
    }
    let size = u32::try_from(image_size).context("vision image_size exceeds image dimensions")?;
    if size == 0 {
        bail!("vision image_size must be positive");
    }

    let (resized_width, resized_height) = if image.width() <= image.height() {
        let height = (u64::from(image.height()) * u64::from(size) + u64::from(image.width()) / 2)
            / u64::from(image.width());
        (
            size,
            u32::try_from(height).context("resized thumbnail height is too large")?,
        )
    } else {
        let width = (u64::from(image.width()) * u64::from(size) + u64::from(image.height()) / 2)
            / u64::from(image.height());
        (
            u32::try_from(width).context("resized thumbnail width is too large")?,
            size,
        )
    };
    let resized = if (resized_width, resized_height) == image.dimensions() {
        std::borrow::Cow::Borrowed(image)
    } else {
        std::borrow::Cow::Owned(image::imageops::resize(
            image,
            resized_width,
            resized_height,
            image::imageops::FilterType::CatmullRom,
        ))
    };
    let crop_x = (resized_width - size) / 2;
    let crop_y = (resized_height - size) / 2;

    let plane_len = image_size
        .checked_mul(image_size)
        .context("vision image_size is too large")?;
    let tensor_len = plane_len
        .checked_mul(3)
        .context("vision input tensor is too large")?;
    let mut tensor = vec![0.0_f32; tensor_len];
    for y in 0..size {
        for x in 0..size {
            let pixel_index = y as usize * image_size + x as usize;
            let pixel = resized.get_pixel(crop_x + x, crop_y + y);
            for channel in 0..3 {
                let unit = f32::from(pixel.0[channel]) / 255.0;
                tensor[channel * plane_len + pixel_index] = (unit - mean[channel]) / std[channel];
            }
        }
    }
    Ok(tensor)
}

#[cfg(feature = "vision-onnx")]
fn probabilities_for_labels(
    labels: &[LabelEmbedding],
    image_embedding: &[f32],
) -> Result<Vec<VisionTag>> {
    let mut logits = Vec::with_capacity(labels.len());
    let mut max_logit = f32::NEG_INFINITY;
    for label in labels {
        let cosine = label
            .embedding
            .iter()
            .zip(image_embedding)
            .map(|(left, right)| left * right)
            .sum::<f32>();
        let logit = cosine * 100.0;
        if !logit.is_finite() {
            bail!("MobileCLIP logit for label `{}` is not finite", label.label);
        }
        max_logit = max_logit.max(logit);
        logits.push(logit);
    }

    let mut denominator = 0.0_f32;
    for logit in &mut logits {
        *logit = (*logit - max_logit).exp();
        denominator += *logit;
    }
    if !denominator.is_finite() || denominator <= 0.0 {
        bail!("MobileCLIP softmax normalization is invalid");
    }

    Ok(labels
        .iter()
        .zip(logits)
        .map(|(label, exponential)| VisionTag {
            label: label.label.clone(),
            score: exponential / denominator,
        })
        .collect())
}

#[cfg(feature = "vision-onnx")]
fn require_explicit_asset(kind: &str, path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path).with_context(|| {
        format!(
            "explicit vision {kind} asset `{}` is unavailable; LumiFlow never downloads vision assets implicitly",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        bail!(
            "explicit vision {kind} asset `{}` is not a file; LumiFlow never downloads vision assets implicitly",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::models::{AnalysisDecision, PhotoAnalysis, PhotoCandidate, TimeSource};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CountingTagger {
        model: String,
        calls: Arc<AtomicUsize>,
        result: Result<Vec<VisionTag>, &'static str>,
    }

    impl CountingTagger {
        fn succeeds(model: &str, calls: Arc<AtomicUsize>, tags: Vec<VisionTag>) -> Self {
            Self {
                model: model.into(),
                calls,
                result: Ok(tags),
            }
        }

        fn fails(model: &str, calls: Arc<AtomicUsize>, message: &'static str) -> Self {
            Self {
                model: model.into(),
                calls,
                result: Err(message),
            }
        }
    }

    impl VisionTagger for CountingTagger {
        fn model_id(&self) -> &str {
            &self.model
        }

        fn tag(&mut self, _thumbnail_path: &Path) -> Result<Vec<VisionTag>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result
                .as_ref()
                .map(Clone::clone)
                .map_err(|message| anyhow::anyhow!(*message))
        }
    }

    fn tag(label: &str, score: f32) -> VisionTag {
        VisionTag {
            label: label.into(),
            score,
        }
    }

    fn db_with_photo() -> TimelineDb {
        let db = TimelineDb::open_in_memory().expect("db");
        let candidate = PhotoCandidate {
            id: "photo".into(),
            relative_path: "photo.jpg".into(),
            filename: "photo.jpg".into(),
            extension: "jpg".into(),
            size_bytes: 100,
            mtime_ns: 1,
            fingerprint: "photo-fp".into(),
            scan_id: "scan".into(),
        };
        assert_eq!(
            db.upsert_candidate(&candidate).expect("candidate"),
            AnalysisDecision::Analyze
        );
        db.save_analysis(&PhotoAnalysis {
            id: "photo".into(),
            taken_at: None,
            time_source: TimeSource::Unknown,
            timezone: None,
            gps_lat: None,
            gps_lon: None,
            width: 10,
            height: 10,
            camera_make: None,
            camera_model: None,
            lens: None,
            exif_json: json!({}),
        })
        .expect("analysis");
        db
    }

    #[test]
    fn unchanged_inputs_reuse_cached_tags_without_calling_tagger_twice() {
        let db = db_with_photo();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tagger =
            CountingTagger::succeeds("model-a", Arc::clone(&calls), vec![tag("family", 0.9)]);

        assert!(matches!(
            tag_photo(
                &db,
                &mut tagger,
                "photo",
                "photo-fp",
                Path::new("thumb.jpg"),
                "thumb-fp",
                "tags-v1",
            )
            .expect("first tag"),
            TagPhotoOutcome::Tagged(_)
        ));
        assert_eq!(
            tag_photo(
                &db,
                &mut tagger,
                "photo",
                "photo-fp",
                Path::new("thumb.jpg"),
                "thumb-fp",
                "tags-v1",
            )
            .expect("cached tag"),
            TagPhotoOutcome::Cached(vec![tag("family", 0.9)])
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn changed_thumbnail_fingerprint_reruns_tagger() {
        let db = db_with_photo();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tagger =
            CountingTagger::succeeds("model-a", Arc::clone(&calls), vec![tag("family", 0.9)]);

        tag_photo(
            &db,
            &mut tagger,
            "photo",
            "photo-fp",
            Path::new("thumb.jpg"),
            "thumb-1",
            "tags-v1",
        )
        .expect("first tag");
        tag_photo(
            &db,
            &mut tagger,
            "photo",
            "photo-fp",
            Path::new("thumb.jpg"),
            "thumb-2",
            "tags-v1",
        )
        .expect("changed thumbnail tag");

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn changed_model_reruns_tagger() {
        let db = db_with_photo();
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let mut first = CountingTagger::succeeds(
            "model-a",
            Arc::clone(&first_calls),
            vec![tag("family", 0.9)],
        );
        let mut second = CountingTagger::succeeds(
            "model-b",
            Arc::clone(&second_calls),
            vec![tag("travel", 0.8)],
        );

        tag_photo(
            &db,
            &mut first,
            "photo",
            "photo-fp",
            Path::new("thumb.jpg"),
            "thumb-fp",
            "tags-v1",
        )
        .expect("first model");
        tag_photo(
            &db,
            &mut second,
            "photo",
            "photo-fp",
            Path::new("thumb.jpg"),
            "thumb-fp",
            "tags-v1",
        )
        .expect("second model");

        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
        assert!(db
            .get_vision_tags("photo", "model-a")
            .expect("first cached model")
            .is_some());
        assert!(db
            .get_vision_tags("photo", "model-b")
            .expect("second cached model")
            .is_some());
    }

    #[test]
    fn failed_refresh_preserves_prior_valid_tags() {
        let db = db_with_photo();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut success =
            CountingTagger::succeeds("model-a", Arc::clone(&calls), vec![tag("family", 0.9)]);
        tag_photo(
            &db,
            &mut success,
            "photo",
            "photo-fp",
            Path::new("thumb.jpg"),
            "thumb-1",
            "tags-v1",
        )
        .expect("first tag");
        let prior = db
            .get_vision_tags("photo", "model-a")
            .expect("cache read")
            .expect("cached tags");
        let mut failure = CountingTagger::fails("model-a", Arc::clone(&calls), "inference failed");

        let error = tag_photo(
            &db,
            &mut failure,
            "photo",
            "photo-fp",
            Path::new("thumb.jpg"),
            "thumb-2",
            "tags-v1",
        )
        .expect_err("refresh must fail");

        assert!(error.to_string().contains("inference failed"));
        let after = db
            .get_vision_tags("photo", "model-a")
            .expect("cache read after error")
            .expect("prior tags remain");
        assert_eq!(after.labels, prior.labels);
        assert_eq!(after.scores, prior.scores);
        assert!(after
            .error
            .as_deref()
            .is_some_and(|error| error.contains("inference failed")));
    }

    #[test]
    fn first_tagging_error_is_recorded_for_future_retry() {
        let db = db_with_photo();
        let mut failure =
            CountingTagger::fails("model-a", Arc::new(AtomicUsize::new(0)), "inference failed");

        assert!(tag_photo(
            &db,
            &mut failure,
            "photo",
            "photo-fp",
            Path::new("thumb.jpg"),
            "thumb-fp",
            "tags-v1",
        )
        .is_err());
        let failed = db
            .get_vision_tags("photo", "model-a")
            .expect("cache read")
            .expect("error row");
        assert!(failed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("inference failed")));
    }

    #[test]
    fn tags_are_validated_sorted_and_limited() {
        let db = db_with_photo();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tagger = CountingTagger::succeeds(
            "model-a",
            calls,
            vec![
                tag("one", 0.1),
                tag("six", 0.6),
                tag("four", 0.4),
                tag("two", 0.2),
                tag("five", 0.5),
                tag("three", 0.3),
            ],
        );

        assert_eq!(
            tag_photo(
                &db,
                &mut tagger,
                "photo",
                "photo-fp",
                Path::new("thumb.jpg"),
                "thumb-fp",
                "tags-v1",
            )
            .expect("tag"),
            TagPhotoOutcome::Tagged(vec![
                tag("six", 0.6),
                tag("five", 0.5),
                tag("four", 0.4),
                tag("three", 0.3),
                tag("two", 0.2),
            ])
        );
    }

    #[test]
    fn invalid_tags_are_rejected_without_writing_cache() {
        for invalid in [
            tag("", 0.5),
            tag("bad", f32::NAN),
            tag("bad", -0.1),
            tag("bad", 1.1),
        ] {
            let db = db_with_photo();
            let mut tagger =
                CountingTagger::succeeds("model-a", Arc::new(AtomicUsize::new(0)), vec![invalid]);
            assert!(tag_photo(
                &db,
                &mut tagger,
                "photo",
                "photo-fp",
                Path::new("thumb.jpg"),
                "thumb-fp",
                "tags-v1",
            )
            .is_err());
            assert_eq!(db.get_vision_tags("photo", "model-a").expect("cache"), None);
        }
    }

    #[test]
    fn disabled_tagger_is_explicit_no_work() {
        assert_eq!(tag_photo_disabled(), TagPhotoOutcome::Disabled);
    }

    #[cfg(feature = "vision-onnx")]
    const TEST_ONNX_BASE64: &str = "CAgSDmx1bWlmbG93LXRlc3RzOosBCjEKBWltYWdlEgllbWJlZGRpbmcaB2ZsYXR0ZW4iB0ZsYXR0ZW4qCwoEYXhpcxgBoAECEhhsdW1pZmxvdy10ZXN0LW1vYmlsZWNsaXBaHwoFaW1hZ2USFgoUCAESEAoCCAEKAggDCgIIAgoCCAJiGwoJZW1iZWRkaW5nEg4KDAgBEggKAggBCgIIDEIECgAQDQ==";

    #[cfg(feature = "vision-onnx")]
    struct OnnxTestAssets {
        dir: std::path::PathBuf,
        model: std::path::PathBuf,
        labels: std::path::PathBuf,
    }

    #[cfg(feature = "vision-onnx")]
    impl OnnxTestAssets {
        fn new() -> Self {
            use base64::Engine as _;

            static NEXT_TEMP_DIR: AtomicUsize = AtomicUsize::new(0);
            let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("lumiflow-vision-{}-{sequence}", std::process::id()));
            std::fs::create_dir(&dir).expect("temp dir");
            let model = dir.join("mobileclip.onnx");
            let labels = dir.join("labels.json");
            let model_bytes = base64::engine::general_purpose::STANDARD
                .decode(TEST_ONNX_BASE64)
                .expect("embedded ONNX base64");
            assert_eq!(model_bytes.len(), 166);
            std::fs::write(&model, model_bytes).expect("ONNX fixture");

            let assets = Self { dir, model, labels };
            assets.write_labels(&valid_label_file());
            assets
        }

        fn write_labels(&self, labels: &serde_json::Value) {
            std::fs::write(
                &self.labels,
                serde_json::to_vec(labels).expect("serialize labels fixture"),
            )
            .expect("labels fixture");
        }
    }

    #[cfg(feature = "vision-onnx")]
    impl Drop for OnnxTestAssets {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).expect("remove temp dir");
        }
    }

    #[cfg(feature = "vision-onnx")]
    fn valid_label_file() -> serde_json::Value {
        json!({
            "version": 1,
            "model_id": "test-mobileclip",
            "tagset_version": "test-tags-v1",
            "image_size": 2,
            "mean": [0.0, 0.0, 0.0],
            "std": [1.0, 1.0, 1.0],
            "labels": [
                { "label": "red", "embedding": [1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
                { "label": "green", "embedding": [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0] }
            ]
        })
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn onnx_tagger_loads_a_valid_explicit_model_and_label_contract() {
        let assets = OnnxTestAssets::new();

        OnnxMobileClipTagger::load(&assets.model, &assets.labels)
            .expect("valid ONNX and label contract must load");
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn onnx_tagger_runs_real_inference_and_returns_softmax_probabilities() {
        let assets = OnnxTestAssets::new();
        let thumbnail = assets.dir.join("red.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]))
            .save(&thumbnail)
            .expect("red PNG fixture");
        let mut tagger =
            OnnxMobileClipTagger::load(&assets.model, &assets.labels).expect("valid ONNX tagger");

        assert_eq!(tagger.model_id(), "test-mobileclip");
        assert_eq!(tagger.tagset_version(), "test-tags-v1");
        let tags = tagger.tag(&thumbnail).expect("real ONNX inference");

        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].label, "red");
        assert_eq!(tags[1].label, "green");
        assert!(tags[0].score > tags[1].score);
        assert!(tags.iter().all(|tag| tag.score.is_finite()));
        let probability_sum: f32 = tags.iter().map(|tag| tag.score).sum();
        assert!(
            (probability_sum - 1.0).abs() < 1e-6,
            "sum={probability_sum}"
        );
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn onnx_tagger_rejects_zero_intra_op_threads() {
        let assets = OnnxTestAssets::new();

        let error = OnnxMobileClipTagger::load_with_threads(&assets.model, &assets.labels, 0)
            .expect_err("zero worker count must fail");

        assert!(format!("{error:#}").contains("threads must be positive"));
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn onnx_tagger_rejects_malformed_label_json_actionably() {
        let assets = OnnxTestAssets::new();
        std::fs::write(&assets.labels, b"not JSON").expect("malformed labels fixture");

        let error = OnnxMobileClipTagger::load(&assets.model, &assets.labels)
            .expect_err("malformed labels must fail");
        let message = format!("{error:#}");
        assert!(message.contains("parse vision labels JSON"), "{message}");
        assert!(message.contains("labels.json"), "{message}");
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn onnx_tagger_rejects_unsupported_label_schema_version() {
        let assets = OnnxTestAssets::new();
        let mut labels = valid_label_file();
        labels["version"] = json!(2);
        assets.write_labels(&labels);

        let error = OnnxMobileClipTagger::load(&assets.model, &assets.labels)
            .expect_err("unsupported schema version must fail");
        let message = format!("{error:#}");
        assert!(message.contains("labels schema version"), "{message}");
        assert!(message.contains("expected 1"), "{message}");
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn onnx_tagger_rejects_json_image_size_disagreement() {
        let assets = OnnxTestAssets::new();
        let mut labels = valid_label_file();
        labels["image_size"] = json!(3);
        assets.write_labels(&labels);

        let error = OnnxMobileClipTagger::load(&assets.model, &assets.labels)
            .expect_err("JSON/model image size disagreement must fail");
        let message = format!("{error:#}");
        assert!(message.contains("image_size"), "{message}");
        assert!(message.contains("3"), "{message}");
        assert!(message.contains("[1, 3, 2, 2]"), "{message}");
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn onnx_tagger_rejects_embedding_output_dimension_disagreement() {
        let assets = OnnxTestAssets::new();
        let mut labels = valid_label_file();
        labels["labels"][0]["embedding"] = json!([1.0, 0.0]);
        labels["labels"][1]["embedding"] = json!([0.0, 1.0]);
        assets.write_labels(&labels);

        let error = OnnxMobileClipTagger::load(&assets.model, &assets.labels)
            .expect_err("embedding/output dimension disagreement must fail");
        let message = format!("{error:#}");
        assert!(message.contains("embedding dimension"), "{message}");
        assert!(message.contains("2"), "{message}");
        assert!(message.contains("12"), "{message}");
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn onnx_tagger_rejects_zero_and_nonfinite_label_vectors() {
        let zero_assets = OnnxTestAssets::new();
        let mut zero_labels = valid_label_file();
        zero_labels["labels"][0]["embedding"] = json!(vec![0.0; 12]);
        zero_assets.write_labels(&zero_labels);
        let error = OnnxMobileClipTagger::load(&zero_assets.model, &zero_assets.labels)
            .expect_err("zero label vector must fail");
        let message = format!("{error:#}");
        assert!(message.contains("nonzero"), "{message}");
        assert!(message.contains("red"), "{message}");

        let mut nonfinite_labels: LabelFile =
            serde_json::from_value(valid_label_file()).expect("typed label fixture");
        nonfinite_labels.labels[0].embedding[0] = f32::NAN;
        let error = validate_label_file(&mut nonfinite_labels)
            .expect_err("nonfinite label vector must fail validation");
        let message = format!("{error:#}");
        assert!(message.contains("finite"), "{message}");
        assert!(message.contains("red"), "{message}");
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn onnx_tagger_rejects_empty_duplicate_labels_and_invalid_preprocessing() {
        let cases = [
            ("model ID must not be empty", {
                let mut value = valid_label_file();
                value["model_id"] = json!(" ");
                value
            }),
            ("tagset version must not be empty", {
                let mut value = valid_label_file();
                value["tagset_version"] = json!("");
                value
            }),
            ("label must not be empty", {
                let mut value = valid_label_file();
                value["labels"][0]["label"] = json!(" ");
                value
            }),
            ("duplicate label", {
                let mut value = valid_label_file();
                value["labels"][1]["label"] = json!("red");
                value
            }),
            ("positive image_size", {
                let mut value = valid_label_file();
                value["image_size"] = json!(0);
                value
            }),
            ("nonzero std", {
                let mut value = valid_label_file();
                value["std"][1] = json!(0.0);
                value
            }),
        ];

        for (expected, labels) in cases {
            let assets = OnnxTestAssets::new();
            assets.write_labels(&labels);
            let error = OnnxMobileClipTagger::load(&assets.model, &assets.labels)
                .expect_err("invalid labels contract must fail");
            let message = format!("{error:#}");
            assert!(
                message.contains(expected),
                "expected `{expected}` in `{message}`"
            );
        }
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn label_contract_rejects_nonfinite_preprocessing_values() {
        let mut labels: LabelFile =
            serde_json::from_value(valid_label_file()).expect("typed label fixture");
        labels.mean[0] = f32::INFINITY;

        let error = validate_label_file(&mut labels)
            .expect_err("nonfinite preprocessing mean must fail validation");
        assert!(format!("{error:#}").contains("finite mean"));
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn label_contract_rejects_empty_labels_and_mismatched_embedding_dimensions() {
        let mut empty: LabelFile =
            serde_json::from_value(valid_label_file()).expect("typed label fixture");
        empty.labels.clear();
        let error = validate_label_file(&mut empty).expect_err("empty labels must fail");
        assert!(format!("{error:#}").contains("at least one label"));

        let mut mismatched: LabelFile =
            serde_json::from_value(valid_label_file()).expect("typed label fixture");
        mismatched.labels[1].embedding.pop();
        let error = validate_label_file(&mut mismatched)
            .expect_err("mismatched embedding dimensions must fail");
        let message = format!("{error:#}");
        assert!(message.contains("green"), "{message}");
        assert!(
            message.contains("embedding dimension 11, expected 12"),
            "{message}"
        );
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn preprocessing_resizes_shortest_side_and_center_crops_deterministically() {
        let image = image::RgbImage::from_fn(4, 2, |x, _| match x {
            0 => image::Rgb([255, 0, 0]),
            1 => image::Rgb([0, 255, 0]),
            2 => image::Rgb([0, 0, 255]),
            _ => image::Rgb([255, 255, 255]),
        });

        let first =
            preprocess_rgb_image(&image, 2, [0.0; 3], [1.0; 3]).expect("first preprocessing");
        let second =
            preprocess_rgb_image(&image, 2, [0.0; 3], [1.0; 3]).expect("second preprocessing");

        assert_eq!(first, second);
        assert_eq!(
            first,
            vec![
                0.0, 0.0, 0.0, 0.0, // red plane from center columns
                1.0, 0.0, 1.0, 0.0, // green plane
                0.0, 1.0, 0.0, 1.0, // blue plane
            ]
        );
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn onnx_tagger_requires_explicit_existing_assets() {
        let missing_model = Path::new("definitely-missing-model.onnx");
        let missing_labels = Path::new("definitely-missing-labels.json");
        let error = OnnxMobileClipTagger::load(missing_model, missing_labels)
            .expect_err("missing assets must fail");
        let message = format!("{error:#}");
        assert!(message.contains("model"));
        assert!(message.contains("definitely-missing-model.onnx"));
        assert!(message.contains("explicit"));
    }
}
