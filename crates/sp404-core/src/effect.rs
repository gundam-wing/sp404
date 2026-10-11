use crate::chain::{ChainDesc, EffectChain, EffectDesc};
use crate::param::EffectSchema;
use crate::Error;

/// A self-contained stereo processor.
///
/// Implementations must not allocate on the steady-state [`Effect::process`]
/// path. Parameters are real-valued and addressed by the ids in
/// [`Effect::schema`]. Bypass is a hard bypass: the buffer is left unchanged
/// and internal state is not updated.
///
/// The trait is object-safe and [`Send`] so a chain can move to an audio
/// thread. It is the extension point for effects that are not in the built-in
/// catalog: implement it, then [`Registry::register`] or [`EffectChain::push`].
pub trait Effect: Send {
    /// Static parameter schema for this instance.
    fn schema(&self) -> &'static EffectSchema;

    /// Process interleaved stereo samples in place.
    ///
    /// `sample_rate` is in hertz. Odd trailing samples are ignored.
    fn process(&mut self, samples: &mut [f32], sample_rate: f32);

    /// Set one parameter. Out-of-range finite values are clamped.
    fn set_param(&mut self, id: &str, value: f32) -> Result<(), Error>;

    /// Current value of a parameter, after clamping.
    fn param(&self, id: &str) -> Option<f32>;

    /// Enable or disable hard bypass.
    fn set_bypassed(&mut self, bypassed: bool);

    /// Whether hard bypass is enabled.
    fn is_bypassed(&self) -> bool;

    /// Clear delay lines, filters, and envelopes. Parameters stay put.
    fn reset(&mut self);
}

#[derive(Clone, Copy)]
pub(crate) struct Factory {
    pub schema: &'static EffectSchema,
    pub create: fn() -> Box<dyn Effect>,
}

/// Resolves effect ids to live processors.
///
/// [`Registry::builtin`] contains the effects shipped with this crate.
/// [`Registry::register`] appends another factory; a repeated id shadows the
/// earlier one. That is how a host can add effects without forking the chain
/// format: the JSON still says `{ "effect": "<id>", "params": { ... } }`.
#[derive(Clone)]
pub struct Registry {
    entries: Vec<Factory>,
}

impl Registry {
    /// Built-in filter, dynamics, lo-fi, pitch, delay, and reverb effects.
    pub fn builtin() -> Self {
        Self {
            entries: crate::effects::FACTORIES.to_vec(),
        }
    }

    /// Register `create` under `schema.id`. A later registration with the same
    /// id takes precedence.
    pub fn register(&mut self, schema: &'static EffectSchema, create: fn() -> Box<dyn Effect>) {
        self.entries.push(Factory { schema, create });
    }

    /// Schemas in registration order, with shadowed ids omitted.
    pub fn schemas(&self) -> Vec<&'static EffectSchema> {
        let mut out: Vec<&'static EffectSchema> = Vec::new();
        for entry in &self.entries {
            if let Some(existing) = out.iter_mut().find(|schema| schema.id == entry.schema.id) {
                *existing = entry.schema;
            } else {
                out.push(entry.schema);
            }
        }
        out
    }

    /// Instantiate `desc`, apply its parameters, and set bypass.
    pub fn create(&self, desc: &EffectDesc) -> Result<Box<dyn Effect>, Error> {
        let factory = self
            .entries
            .iter()
            .rev()
            .find(|entry| entry.schema.id == desc.effect)
            .ok_or_else(|| Error::UnknownEffect {
                id: desc.effect.clone(),
            })?;
        let mut effect = (factory.create)();
        effect.set_bypassed(desc.bypass);
        for (id, value) in &desc.params {
            effect.set_param(id, *value)?;
        }
        Ok(effect)
    }

    /// Build a chain. Instantiation stops at the first unknown effect or parameter.
    pub fn chain(&self, desc: &ChainDesc) -> Result<EffectChain, Error> {
        let mut chain = EffectChain::new();
        for effect_desc in &desc.effects {
            chain.push(self.create(effect_desc)?);
        }
        Ok(chain)
    }
}

/// Schemas of the built-in effects, in catalog order.
pub fn effect_schemas() -> Vec<&'static EffectSchema> {
    Registry::builtin().schemas()
}

/// Look up one built-in schema by id.
pub fn effect_schema(id: &str) -> Option<&'static EffectSchema> {
    effect_schemas().into_iter().find(|schema| schema.id == id)
}

/// Create one built-in effect from a description.
pub fn create_effect(desc: &EffectDesc) -> Result<Box<dyn Effect>, Error> {
    Registry::builtin().create(desc)
}

macro_rules! impl_effect_accessors {
    () => {
        fn schema(&self) -> &'static $crate::param::EffectSchema {
            self.params.schema()
        }
        fn set_param(&mut self, id: &str, value: f32) -> Result<(), $crate::Error> {
            self.params.set(id, value)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.params.get(id)
        }
        fn set_bypassed(&mut self, bypassed: bool) {
            self.params.set_bypassed(bypassed);
        }
        fn is_bypassed(&self) -> bool {
            self.params.is_bypassed()
        }
    };
}

pub(crate) use impl_effect_accessors;
