//! Built-in insert effects.
//!
//! Each module owns its schema, its state, and a `create` factory. The list
//! in [`FACTORIES`] is the catalog order.

mod compressor;
mod delay;
mod filter;
mod gain;
mod isolator;
mod lofi;
mod pitch;
mod reverb;
mod vinyl;

use crate::effect::Factory;

pub(crate) static FACTORIES: [Factory; 9] = [
    Factory {
        schema: &gain::SCHEMA,
        create: gain::create,
    },
    Factory {
        schema: &filter::SCHEMA,
        create: filter::create,
    },
    Factory {
        schema: &isolator::SCHEMA,
        create: isolator::create,
    },
    Factory {
        schema: &compressor::SCHEMA,
        create: compressor::create,
    },
    Factory {
        schema: &lofi::SCHEMA,
        create: lofi::create,
    },
    Factory {
        schema: &vinyl::SCHEMA,
        create: vinyl::create,
    },
    Factory {
        schema: &pitch::SCHEMA,
        create: pitch::create,
    },
    Factory {
        schema: &delay::SCHEMA,
        create: delay::create,
    },
    Factory {
        schema: &reverb::SCHEMA,
        create: reverb::create,
    },
];
