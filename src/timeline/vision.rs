// Stub — ONNX vision tagging removed.
use crate::timeline::db::TimelineDb;
use anyhow::{bail, Result};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct VisionTag {
    pub label: String,
    pub score: f32,
}

pub trait VisionTagger {
    fn model_id(&self) -> &str;
    fn tag(&mut self, _thumbnail_path: &Path) -> Result<Vec<VisionTag>>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum TagPhotoOutcome {
    Disabled,
    Cached(Vec<VisionTag>),
    Tagged(Vec<VisionTag>),
}

pub fn tag_photo<T: VisionTagger + ?Sized>(
    _db: &TimelineDb,
    tagger: &mut T,
    _photo_id: &str,
    _photo_fingerprint: &str,
    thumbnail_path: &Path,
    _thumbnail_fingerprint: &str,
    _tagset_version: &str,
) -> Result<TagPhotoOutcome> {
    let tags = tagger.tag(thumbnail_path)?;
    Ok(TagPhotoOutcome::Tagged(tags))
}
