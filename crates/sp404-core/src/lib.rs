//! Pad404 core: a pad sampler and a rack of self-contained effects.
//!
//! This crate is the portable heart of the sampler. It has no audio-device
//! I/O, file dialogs, or threads. The same code path renders a buffer in a
//! native UI callback, a CLI, or a `wasm32-unknown-unknown` build.
//!
//! # Effects
//!
//! Each effect is a [`Effect`] with a static [`EffectSchema`]. Schemas name
//! every parameter and publish its range, unit, curve, and default. The
//! built-in set is listed by [`effect_schemas`] and, as one JSON document, by
//! [`catalog_json`].
//!
//! Effects compose by being placed in an [`EffectChain`]. A chain round-trips
//! through [`ChainDesc`], which is plain JSON:
//!
//! ```json
//! {
//!   "effects": [
//!     { "effect": "lofi", "params": { "bit_depth": 6, "downsample": 3, "mix": 1.0 } },
//!     { "effect": "delay", "params": { "time_ms": 180, "feedback": 0.35, "mix": 0.3 } }
//!   ]
//! }
//! ```
//!
//! # Headless render
//!
//! [`render_json`] runs a buffer through a chain description and returns
//! samples. [`encode_wav`] turns those samples into a WAV byte string. Neither
//! function touches the filesystem.
//!
//! ```
//! use sp404_core::{render_json, AudioBuffer};
//! let kick = AudioBuffer::demo("kick", 44_100.0).unwrap();
//! let chain = r#"{"effects":[{"effect":"gain","params":{"gain_db":-6.0}}]}"#;
//! let out = render_json(&kick, chain).unwrap();
//! assert_eq!(out.frames(), kick.frames());
//! assert!(out.peak() < kick.peak());
//! ```
//!
//! # Extending
//!
//! Implement [`Effect`] and [`Registry::register`] it, or push a boxed effect
//! onto an [`EffectChain`] directly. JSON chains resolve ids through a
//! [`Registry`]; [`Registry::builtin`] knows the effects shipped here.
//!
//! An out-of-process agent (CLI today, MCP or another host later) only needs
//! `catalog_json`, `ChainDesc`, `render` / `render_json`, and `encode_wav`.
//! The sampler UI is a separate client of [`Engine`].
//!
//! # Recording
//!
//! [`ingest_take`] turns interleaved frames into a stereo [`AudioBuffer`] at
//! the engine rate. The host records those frames. This crate does not open a
//! microphone, so a browser shell can pass the same buffers from an
//! AudioWorklet.

#![forbid(unsafe_code)]

mod buffer;
mod capture;
mod chain;
mod demos;
mod dsp;
mod effect;
mod error;
mod param;
pub mod plan;
mod presets;
mod render;
mod sampler;
mod state;
mod vary;
mod wav;

mod effects;

pub use buffer::AudioBuffer;
pub use capture::{ingest_take, Take, TakeOptions};
pub use chain::{ChainDesc, EffectChain, EffectDesc};
pub use effect::{create_effect, effect_schema, effect_schemas, Effect, Registry};
pub use error::Error;
pub use param::{EffectSchema, ParamScale, ParamSchema};
pub use plan::{
    Arrangement, Automation, BusPlan, Ease, Keyframe, LfoShape, PadPlan, PlanBuilder, PlanRenderer,
    PlanTime, SampleSource, Target, Track, Trigger, ARRANGEMENT_VERSION, CHECKPOINT_FRAMES,
};
pub use presets::{chain_presets, ChainPreset};
pub use render::{render, render_json, render_performance, NoteEvent};
pub use sampler::{Engine, PadView, PAD_COUNT};
pub use state::EffectState;
pub use vary::{analyze, vary, Analysis, Phrase, Variation, VaryRequest};
pub use wav::{decode_wav, encode_wav};

/// Schema document version emitted by [`catalog_json`].
pub const CATALOG_VERSION: u32 = 1;

/// Machine-readable catalog: effect schemas plus the built-in chain presets.
///
/// Agents should start here. `effects[].id` is the string used in chain JSON.
/// Each parameter lists `min`, `max`, `default`, `unit`, and `scale`
/// (`linear`, `logarithmic`, or `choice`).
pub fn catalog_json() -> String {
    let mut effects = serde_json::to_value(effect_schemas()).expect("schemas are finite");
    if let Some(list) = effects.as_array_mut() {
        for effect in list {
            let Some(params) = effect
                .get_mut("params")
                .and_then(|value| value.as_array_mut())
            else {
                continue;
            };
            for param in params {
                if let Some(object) = param.as_object_mut() {
                    object.insert("automatable".to_string(), serde_json::Value::Bool(true));
                }
            }
        }
    }
    let presets: Vec<serde_json::Value> = chain_presets()
        .iter()
        .map(|preset| {
            let chain: serde_json::Value = serde_json::from_str(preset.chain_json)
                .unwrap_or_else(|_| serde_json::Value::String(preset.chain_json.to_string()));
            serde_json::json!({
                "id": preset.id,
                "name": preset.name,
                "description": preset.description,
                "chain": chain,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "version": CATALOG_VERSION,
        "effects": effects,
        "presets": presets,
        "arrangement": plan::arrangement_catalog(),
    });
    serde_json::to_string_pretty(&doc).expect("catalog is static and finite")
}

/// Built-in chain presets as a JSON array.
pub fn presets_json() -> String {
    let presets: Vec<serde_json::Value> = chain_presets()
        .iter()
        .map(|preset| {
            let chain: serde_json::Value =
                serde_json::from_str(preset.chain_json).expect("preset chain is valid json");
            serde_json::json!({
                "id": preset.id,
                "name": preset.name,
                "description": preset.description,
                "chain": chain,
            })
        })
        .collect();
    serde_json::to_string_pretty(&presets).expect("presets are static and finite")
}
