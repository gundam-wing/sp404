//! Krumhansl–Schmuckler key finding.
//!
//! Chroma is correlated with the Krumhansl–Kessler probe-tone profiles
//! (Krumhansl & Kessler, 1982) rotated through all 24 major and minor keys.
//! The winning Pearson correlation, clamped to 0..1, is the confidence.

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Major profile, tonic at index 0.
const MAJOR: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];

/// Minor profile, tonic at index 0.
const MINOR: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Key name (`"C major"`, `"F# minor"`, or `"unknown"`) and confidence in 0..1.
pub fn match_chroma(chroma: &[f32; 12]) -> (String, f64) {
    let energy: f32 = chroma.iter().copied().sum();
    if energy < 1.0e-6 {
        return ("unknown".to_string(), 0.0);
    }
    let mut best_r = f64::NEG_INFINITY;
    let mut best_name = "C major".to_string();
    for (profile, mode) in [(MAJOR, "major"), (MINOR, "minor")] {
        for (tonic, name) in NAMES.iter().enumerate() {
            let rotated = rotate(&profile, tonic);
            let score = pearson(chroma, &rotated);
            if score > best_r {
                best_r = score;
                best_name = format!("{name} {mode}");
            }
        }
    }
    let confidence = if best_r.is_finite() {
        best_r.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (best_name, confidence)
}

fn rotate(profile: &[f32; 12], tonic: usize) -> [f32; 12] {
    let mut out = [0.0f32; 12];
    for (degree, weight) in profile.iter().copied().enumerate() {
        out[(degree + tonic) % 12] = weight;
    }
    out
}

fn pearson(chroma: &[f32; 12], profile: &[f32; 12]) -> f64 {
    let mean_chroma = chroma.iter().map(|value| f64::from(*value)).sum::<f64>() / 12.0;
    let mean_profile = profile.iter().map(|value| f64::from(*value)).sum::<f64>() / 12.0;
    let mut num = 0.0;
    let mut chroma_energy = 0.0;
    let mut profile_energy = 0.0;
    for i in 0..12 {
        let left = f64::from(chroma[i]) - mean_chroma;
        let right = f64::from(profile[i]) - mean_profile;
        num += left * right;
        chroma_energy += left * left;
        profile_energy += right * right;
    }
    if chroma_energy <= 1.0e-12 || profile_energy <= 1.0e-12 {
        return 0.0;
    }
    num / (chroma_energy.sqrt() * profile_energy.sqrt())
}

#[cfg(test)]
mod tests {
    use super::match_chroma;

    #[test]
    fn major_triad_profile_prefers_c_major() {
        let mut chroma = [0.0f32; 12];
        chroma[0] = 1.0;
        chroma[4] = 0.8;
        chroma[7] = 0.9;
        let (name, confidence) = match_chroma(&chroma);
        assert_eq!(name, "C major");
        assert!(confidence > 0.6, "confidence {confidence}");
    }

    #[test]
    fn major_scale_profile_prefers_c_major_over_the_relative_minor() {
        let mut chroma = [0.0f32; 12];
        for class in [0, 2, 4, 5, 7, 9, 11] {
            chroma[class] = 1.0;
        }
        let (name, confidence) = match_chroma(&chroma);
        assert_eq!(name, "C major");
        assert!(confidence > 0.5, "confidence {confidence}");
    }
}
