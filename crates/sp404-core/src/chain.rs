use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dsp::sane_sr;
use crate::effect::{Effect, Registry};
use crate::Error;

/// One effect slot in a serializable chain.
///
/// Omitted parameters keep the schema default. Finite values outside the
/// schema range are clamped when the slot is instantiated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectDesc {
    /// Effect type id from the catalog. `id` and `kind` are accepted aliases.
    #[serde(alias = "id", alias = "kind")]
    pub effect: String,
    /// Hard bypass. The slot stays in the chain but does not touch audio.
    #[serde(default)]
    pub bypass: bool,
    /// Parameter id to value, in real units.
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
}

/// A serializable series of effects.
///
/// JSON may be either this object (`{"effects":[...]}`) or a bare array of
/// effect objects. [`ChainDesc::to_json`] always writes the object form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ChainDesc {
    /// Slots, processed from first to last.
    #[serde(default)]
    pub effects: Vec<EffectDesc>,
}

impl ChainDesc {
    /// Parse a chain object or a bare array of effect objects.
    pub fn from_json(text: &str) -> Result<Self, Error> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|err| Error::Json(err.to_string()))?;
        if value.is_array() {
            let effects =
                serde_json::from_value(value).map_err(|err| Error::Json(err.to_string()))?;
            return Ok(Self { effects });
        }
        serde_json::from_value(value).map_err(|err| Error::Json(err.to_string()))
    }

    /// Pretty JSON object form.
    pub fn to_json(&self) -> Result<String, Error> {
        serde_json::to_string_pretty(self).map_err(|err| Error::Json(err.to_string()))
    }

    /// Instantiate this description with the built-in registry.
    pub fn instantiate(&self) -> Result<EffectChain, Error> {
        Registry::builtin().chain(self)
    }
}

/// A live series of effects.
///
/// Processing is in-place and serial. An empty chain leaves the buffer
/// unchanged. The chain owns its processors; parameter edits do not rebuild
/// them, so delay and reverb tails survive a slider move.
pub struct EffectChain {
    effects: Vec<Box<dyn Effect>>,
}

impl EffectChain {
    /// An empty chain. It passes audio through unchanged.
    pub fn new() -> Self {
        Self {
            effects: Vec::new(),
        }
    }

    /// Append a processor. Use this for effects that are not in the JSON catalog.
    pub fn push(&mut self, effect: Box<dyn Effect>) {
        self.effects.push(effect);
    }

    /// Number of slots.
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    /// Whether the chain has no slots.
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// Schema id of the slot at `index`.
    pub fn effect_id(&self, index: usize) -> Option<&str> {
        self.effects.get(index).map(|effect| effect.schema().id)
    }

    /// Run every slot in order. Invalid sample rates are treated as 44.1 kHz.
    pub fn process(&mut self, samples: &mut [f32], sample_rate: f32) {
        let sample_rate = sane_sr(sample_rate);
        for effect in &mut self.effects {
            effect.process(samples, sample_rate);
        }
    }

    /// Set one parameter on one slot without rebuilding the chain.
    pub fn set_param(&mut self, index: usize, id: &str, value: f32) -> Result<(), Error> {
        self.effects
            .get_mut(index)
            .ok_or(Error::EffectOutOfRange { index })?
            .set_param(id, value)
    }

    /// Hard-bypass one slot.
    pub fn set_bypassed(&mut self, index: usize, bypassed: bool) -> Result<(), Error> {
        let effect = self
            .effects
            .get_mut(index)
            .ok_or(Error::EffectOutOfRange { index })?;
        effect.set_bypassed(bypassed);
        Ok(())
    }

    /// Clear state on every slot.
    pub fn reset(&mut self) {
        for effect in &mut self.effects {
            effect.reset();
        }
    }

    /// Snapshot the live parameters back into a description.
    pub fn to_desc(&self) -> ChainDesc {
        let effects = self
            .effects
            .iter()
            .map(|effect| {
                let mut params = BTreeMap::new();
                for spec in effect.schema().params {
                    if let Some(value) = effect.param(spec.id) {
                        params.insert(spec.id.to_string(), value);
                    }
                }
                EffectDesc {
                    effect: effect.schema().id.to_string(),
                    bypass: effect.is_bypassed(),
                    params,
                }
            })
            .collect();
        ChainDesc { effects }
    }
}

impl Default for EffectChain {
    fn default() -> Self {
        Self::new()
    }
}
