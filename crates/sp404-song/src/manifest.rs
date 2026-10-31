//! JSON manifest written by `sp404 chop` and read by `sp404 generate`.

use serde::{Deserialize, Serialize};

use crate::error::{err, Error};
use crate::label::SliceLabel;

/// Document version written by [`Manifest::to_json`].
pub const MANIFEST_VERSION: u32 = 1;

/// One chopped loop: slice audio paths plus the analysis the generator needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Document version. Only [`MANIFEST_VERSION`] is accepted.
    pub version: u32,
    /// Path of the WAV that was chopped, as the CLI received it.
    pub source: String,
    /// `transient`, `beat-grid`, or `phrase`.
    pub mode: String,
    /// Sample rate of the slice WAVs, in hertz.
    pub sample_rate: f64,
    /// Source length in seconds.
    pub duration: f64,
    /// Tempo copied from analysis. `0` means none was found.
    pub bpm: f64,
    /// Key name copied from analysis.
    pub key: String,
    /// Key confidence copied from analysis, in 0..1.
    pub key_confidence: f64,
    /// Bars per phrase. `null` unless `mode` is `phrase`.
    pub phrase_bars: Option<u32>,
    /// Slices in time order. `path` is relative to this file.
    pub slices: Vec<ManifestSlice>,
}

/// One slice in a [`Manifest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestSlice {
    /// Index in `slices`, also the WAV filename stem.
    pub index: usize,
    /// `kick`, `snare`, `hat`, or `melodic`.
    pub label: String,
    /// Start in the source, seconds.
    pub start: f64,
    /// End in the source, seconds.
    pub end: f64,
    /// Fade-in applied to the WAV, seconds.
    pub fade_in: f64,
    /// Fade-out applied to the WAV, seconds.
    pub fade_out: f64,
    /// WAV path relative to the manifest.
    pub path: String,
}

impl Manifest {
    /// Parse a chop manifest and check the fields generate needs.
    pub fn from_json(text: &str) -> Result<Self, Error> {
        let manifest: Self = serde_json::from_str(text)
            .map_err(|parse| err(format!("invalid chop manifest: {parse}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("manifest values are finite")
    }

    fn validate(&self) -> Result<(), Error> {
        if self.version != MANIFEST_VERSION {
            return Err(err(format!(
                "unsupported manifest version {} (this crate reads version {MANIFEST_VERSION})",
                self.version
            )));
        }
        if self.slices.is_empty() {
            return Err(err("manifest has no slices"));
        }
        if !(self.bpm.is_finite() && self.bpm > 0.0 && self.bpm <= 1_000.0) {
            return Err(err(
                "manifest bpm must be finite, above 0, and at most 1000",
            ));
        }
        if !(self.sample_rate.is_finite() && (8_000.0..=192_000.0).contains(&self.sample_rate)) {
            return Err(err("manifest sample rate must be from 8000 to 192000 Hz"));
        }
        for slice in &self.slices {
            if slice.path.is_empty() || slice.path.contains('\0') {
                return Err(err(format!("slice {} has an empty path", slice.index)));
            }
            if SliceLabel::parse(&slice.label).is_none() {
                return Err(err(format!(
                    "slice {} has unknown label '{}'",
                    slice.index, slice.label
                )));
            }
        }
        Ok(())
    }
}

/// `target` relative to `base_dir`, using `..` for parents.
///
/// Both paths should already be absolute. This does not touch the filesystem.
pub fn relative_to(base_dir: &std::path::Path, target: &std::path::Path) -> std::path::PathBuf {
    let base: Vec<_> = base_dir.components().collect();
    let target: Vec<_> = target.components().collect();
    let mut shared = 0;
    while shared < base.len() && shared < target.len() && base[shared] == target[shared] {
        shared += 1;
    }
    let mut out = std::path::PathBuf::new();
    for _ in shared..base.len() {
        out.push("..");
    }
    for component in &target[shared..] {
        out.push(component);
    }
    if out.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::relative_to;
    use std::path::Path;

    #[test]
    fn relative_paths_stay_beside_the_plan() {
        let plan = Path::new("/tmp/song");
        let slice = Path::new("/tmp/song/chops/slices/000.wav");
        assert_eq!(relative_to(plan, slice), Path::new("chops/slices/000.wav"));
        assert_eq!(
            relative_to(Path::new("/tmp/song/chops"), slice),
            Path::new("slices/000.wav")
        );
    }
}
