//! Chop an analyzed loop into labeled slices, then generate an arrangement.
//!
//! This crate is native-only. `sp404-core` does not depend on it, so the core
//! stays free of file IO, `rustfft`, and analysis. Chopping reads samples the
//! caller already decoded. Generating returns an [`Arrangement`] the existing
//! renderer accepts.
//!
//! The same samples, mode, style, and seed always produce the same slices and
//! the same arrangement JSON.

#![forbid(unsafe_code)]

mod chop;
mod error;
mod euclidean;
mod generate;
mod label;
mod manifest;
mod rng;

pub use chop::{chop, ChopMode, ChopSource, Chopped, ChoppedSlice, SliceMark};
pub use error::Error;
pub use euclidean::{euclidean, hit_steps, rotate};
pub use generate::{generate, swung_step, GenerateOptions, PadVoice, Style};
pub use label::{label_slice, SliceLabel};
pub use manifest::{relative_to, Manifest, ManifestSlice, MANIFEST_VERSION};

/// Manifest for `chopped`, with WAV paths `slices/000.wav` and so on.
///
/// Paths are relative to the manifest file. The CLI writes the audio beside it.
pub fn manifest_for(chopped: &Chopped, source: impl Into<String>) -> Manifest {
    Manifest {
        version: MANIFEST_VERSION,
        source: source.into(),
        mode: chopped.mode.as_str().to_string(),
        sample_rate: chopped.sample_rate,
        duration: chopped.duration,
        bpm: chopped.bpm,
        key: chopped.key.clone(),
        key_confidence: chopped.key_confidence,
        phrase_bars: chopped.mode.phrase_bars(),
        slices: chopped
            .slices
            .iter()
            .map(|slice| ManifestSlice {
                index: slice.index,
                label: slice.label.as_str().to_string(),
                start: slice.start,
                end: slice.end,
                fade_in: slice.fade_in,
                fade_out: slice.fade_out,
                path: format!("slices/{:03}.wav", slice.index),
            })
            .collect(),
    }
}
