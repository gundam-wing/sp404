//! Chain presets shipped as JSON so the UI, the CLI, and an agent share them.
//!
//! The files under `examples/chains/` are the source. They are embedded, so a
//! wasm or library caller does not need the files on disk.

/// A named chain an agent or the UI can drop onto a pad or the bus.
#[derive(Debug, Clone, Copy)]
pub struct ChainPreset {
    /// Stable id.
    pub id: &'static str,
    /// Short label.
    pub name: &'static str,
    /// What the chain is for.
    pub description: &'static str,
    /// Chain JSON. Either an object or a bare effect array.
    pub chain_json: &'static str,
}

const PRESETS: &[ChainPreset] = &[
    ChainPreset {
        id: "dry",
        name: "Dry",
        description: "No effects.",
        chain_json: include_str!("../../../examples/chains/dry.json"),
    },
    ChainPreset {
        id: "lofi-phone",
        name: "Lo-Fi Phone",
        description: "Band-limited and bit-crushed, like a small speaker.",
        chain_json: include_str!("../../../examples/chains/lofi-phone.json"),
    },
    ChainPreset {
        id: "dub-delay",
        name: "Dub Delay",
        description: "Filtered slapback into a short room.",
        chain_json: include_str!("../../../examples/chains/dub-delay.json"),
    },
    ChainPreset {
        id: "vinyl",
        name: "Vinyl",
        description: "Wow, crackle, and a gentle isolator tilt.",
        chain_json: include_str!("../../../examples/chains/vinyl.json"),
    },
    ChainPreset {
        id: "pitched",
        name: "Pitched Echo",
        description: "A fifth down into a short delay.",
        chain_json: include_str!("../../../examples/chains/pitched.json"),
    },
    ChainPreset {
        id: "room",
        name: "Room",
        description: "A small reverb for the bus.",
        chain_json: include_str!("../../../examples/chains/room.json"),
    },
    ChainPreset {
        id: "crush-space",
        name: "Crush Space",
        description: "Heavy sample-rate reduction into a wide reverb.",
        chain_json: include_str!("../../../examples/chains/crush-space.json"),
    },
];

/// Built-in chain presets.
pub fn chain_presets() -> &'static [ChainPreset] {
    PRESETS
}
