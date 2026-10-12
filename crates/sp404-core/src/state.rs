//! Exact processor memory for deterministic checkpoints.
//!
//! The layout is an implementation detail of this crate version. Arrangements
//! do not store it. A renderer rebuilds it by replaying the plan.

/// Snapshot of one effect's delay lines, filter memory, and counters.
///
/// `f32` values are stored as written, including their bits. Two snapshots
/// compare equal only when every bit matches.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EffectState {
    /// Filter memories, phases, and delay-line samples.
    pub floats: Vec<f32>,
    /// Write heads, lengths, and integer counters.
    pub ints: Vec<u32>,
}

impl EffectState {
    /// A stateless effect's snapshot.
    pub fn is_empty(&self) -> bool {
        self.floats.is_empty() && self.ints.is_empty()
    }
}

pub(crate) struct Writer {
    floats: Vec<f32>,
    ints: Vec<u32>,
}

impl Writer {
    pub(crate) fn new() -> Self {
        Self {
            floats: Vec::new(),
            ints: Vec::new(),
        }
    }

    pub(crate) fn f(&mut self, value: f32) {
        self.floats.push(value);
    }

    pub(crate) fn u(&mut self, value: u32) {
        self.ints.push(value);
    }

    pub(crate) fn samples(&mut self, values: &[f32]) {
        self.floats.extend_from_slice(values);
    }

    pub(crate) fn finish(self) -> EffectState {
        EffectState {
            floats: self.floats,
            ints: self.ints,
        }
    }
}

pub(crate) struct Reader<'a> {
    floats: &'a [f32],
    ints: &'a [u32],
    float_at: usize,
    int_at: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(state: &'a EffectState) -> Self {
        Self {
            floats: &state.floats,
            ints: &state.ints,
            float_at: 0,
            int_at: 0,
        }
    }

    pub(crate) fn f(&mut self) -> Option<f32> {
        let value = self.floats.get(self.float_at).copied()?;
        self.float_at += 1;
        Some(value)
    }

    pub(crate) fn u(&mut self) -> Option<u32> {
        let value = self.ints.get(self.int_at).copied()?;
        self.int_at += 1;
        Some(value)
    }

    pub(crate) fn samples(&mut self, len: usize) -> Option<&[f32]> {
        let end = self.float_at.checked_add(len)?;
        let slice = self.floats.get(self.float_at..end)?;
        self.float_at = end;
        Some(slice)
    }

    pub(crate) fn finish(self) -> bool {
        self.float_at == self.floats.len() && self.int_at == self.ints.len()
    }
}
