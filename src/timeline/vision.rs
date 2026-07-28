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

pub fn tag_photo<T: VisionTagger>(
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

    let tags = tagger
        .tag(thumbnail_path)
        .map_err(|error| anyhow::anyhow!("vision tagger failed: {error:#}"))?;
    let tags = normalize_tags(tags)?;
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
#[derive(Debug)]
pub struct OnnxMobileClipTagger {
    _private: (),
}

#[cfg(feature = "vision-onnx")]
impl OnnxMobileClipTagger {
    pub fn load(model_path: &Path, labels_path: &Path) -> Result<Self> {
        require_explicit_asset("model", model_path)?;
        require_explicit_asset("labels", labels_path)?;

        bail!(
            "the MobileCLIP tensor contract for `{}` is not configured; provide a supported model export contract before enabling ONNX vision (labels: `{}`); no inference was attempted and no assets were downloaded",
            model_path.display(),
            labels_path.display()
        )
    }
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
        assert_eq!(
            db.get_vision_tags("photo", "model-a")
                .expect("cache read after error"),
            Some(prior)
        );
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
    #[test]
    fn onnx_tagger_requires_explicit_existing_assets() {
        let missing_model = Path::new("definitely-missing-model.onnx");
        let missing_labels = Path::new("definitely-missing-labels.txt");
        let error = OnnxMobileClipTagger::load(missing_model, missing_labels)
            .expect_err("missing assets must fail");
        let message = format!("{error:#}");
        assert!(message.contains("model"));
        assert!(message.contains("definitely-missing-model.onnx"));
        assert!(message.contains("explicit"));
    }

    #[cfg(feature = "vision-onnx")]
    #[test]
    fn onnx_tagger_rejects_unimplemented_tensor_contract_actionably() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("lumiflow-vision-{nonce}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let model = dir.join("mobileclip.onnx");
        let labels = dir.join("labels.txt");
        std::fs::write(&model, b"not a model").expect("model fixture");
        std::fs::write(&labels, b"family\ntravel\n").expect("labels fixture");

        let error = OnnxMobileClipTagger::load(&model, &labels)
            .expect_err("unsupported tensor contract must fail");
        let message = format!("{error:#}");
        assert!(message.contains("MobileCLIP tensor contract"));
        assert!(message.contains("no inference was attempted"));

        std::fs::remove_dir_all(dir).expect("remove temp dir");
    }
}
