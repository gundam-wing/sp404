use serde::Serialize;

/// How a UI or agent should space values between `min` and `max`.
///
/// Stored values are always in real units (hertz, decibels, milliseconds).
/// `Logarithmic` is only a hint for controls. `Choice` values are integer
/// indices into [`ParamSchema::choices`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamScale {
    /// Evenly spaced in the unit of the parameter.
    Linear,
    /// Evenly spaced in log space. `min` and `max` are positive.
    Logarithmic,
    /// Integer index into [`ParamSchema::choices`].
    Choice,
}

/// One parameter on an effect, in the form an agent can validate against.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ParamSchema {
    /// Stable id used as the key in chain JSON.
    pub id: &'static str,
    /// Short label for humans.
    pub name: &'static str,
    /// What the parameter does.
    pub description: &'static str,
    /// Unit suffix, such as `Hz`, `dB`, `ms`, or `st`. Empty if dimensionless.
    pub unit: &'static str,
    /// Inclusive lower bound in real units.
    pub min: f32,
    /// Inclusive upper bound in real units.
    pub max: f32,
    /// Value used when a chain omits this key.
    pub default: f32,
    /// Control curve. Does not change the stored unit.
    pub scale: ParamScale,
    /// Labels for [`ParamScale::Choice`], index-aligned with `min..=max`.
    /// Empty for continuous parameters.
    pub choices: &'static [&'static str],
}

/// Static description of one effect type.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct EffectSchema {
    /// Stable id used in chain JSON, for example `filter`.
    pub id: &'static str,
    /// Human name.
    pub name: &'static str,
    /// One-paragraph description of the processor.
    pub description: &'static str,
    /// Parameters in display and storage order.
    pub params: &'static [ParamSchema],
}

impl EffectSchema {
    /// Clamp `value` into range. Choice parameters snap to the nearest index.
    pub fn clamp_param(&self, id: &str, value: f32) -> Option<f32> {
        let spec = self.params.iter().find(|param| param.id == id)?;
        Some(normalize(spec, value))
    }
}

pub(crate) fn normalize(spec: &ParamSchema, value: f32) -> f32 {
    let mut value = value.clamp(spec.min, spec.max);
    if spec.scale == ParamScale::Choice {
        value = value.round().clamp(spec.min, spec.max);
    }
    value
}

/// Owned parameter bag shared by the built-in effects.
pub(crate) struct ParamBank {
    schema: &'static EffectSchema,
    values: Vec<f32>,
    bypassed: bool,
}

impl ParamBank {
    pub(crate) fn new(schema: &'static EffectSchema) -> Self {
        let values = schema.params.iter().map(|param| param.default).collect();
        Self {
            schema,
            values,
            bypassed: false,
        }
    }

    pub(crate) fn schema(&self) -> &'static EffectSchema {
        self.schema
    }

    pub(crate) fn value(&self, index: usize) -> f32 {
        self.values[index]
    }

    pub(crate) fn get(&self, id: &str) -> Option<f32> {
        self.schema
            .params
            .iter()
            .position(|param| param.id == id)
            .map(|index| self.values[index])
    }

    pub(crate) fn set(&mut self, id: &str, value: f32) -> Result<(), crate::Error> {
        let Some(index) = self.schema.params.iter().position(|param| param.id == id) else {
            return Err(crate::Error::UnknownParam {
                effect: self.schema.id.to_string(),
                param: id.to_string(),
            });
        };
        if !value.is_finite() {
            return Err(crate::Error::InvalidValue {
                effect: self.schema.id.to_string(),
                param: id.to_string(),
                reason: "value must be finite".to_string(),
            });
        }
        self.values[index] = normalize(&self.schema.params[index], value);
        Ok(())
    }

    pub(crate) fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
    }

    pub(crate) fn is_bypassed(&self) -> bool {
        self.bypassed
    }
}
