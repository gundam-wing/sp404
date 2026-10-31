//! Bjorklund Euclidean rhythms.
//!
//! `euclidean(pulses, steps)` is the Bjorklund pattern used by Toussaint.
//! The rotation is left to the caller so a style can put a hit on the
//! downbeat or the backbeat.

/// Distribute `pulses` across `steps`.
///
/// `pulses` is clamped to `steps`. Zero steps returns an empty pattern.
/// Zero pulses returns `steps` rests.
pub fn euclidean(pulses: usize, steps: usize) -> Vec<bool> {
    if steps == 0 {
        return Vec::new();
    }
    let pulses = pulses.min(steps);
    if pulses == 0 {
        return vec![false; steps];
    }

    let mut counts = Vec::new();
    let mut remainders = vec![pulses];
    let mut divisor = steps - pulses;
    let mut level = 0usize;
    loop {
        counts.push(divisor / remainders[level]);
        remainders.push(divisor % remainders[level]);
        divisor = remainders[level];
        level += 1;
        if remainders[level] <= 1 {
            break;
        }
    }
    counts.push(divisor);

    let mut pattern = Vec::with_capacity(steps);
    build(level as isize, &counts, &remainders, &mut pattern);
    pattern.truncate(steps);
    debug_assert_eq!(pattern.len(), steps);
    pattern
}

fn build(level: isize, counts: &[usize], remainders: &[usize], pattern: &mut Vec<bool>) {
    if level == -1 {
        pattern.push(false);
    } else if level == -2 {
        pattern.push(true);
    } else {
        let index = level as usize;
        for _ in 0..counts[index] {
            build(level - 1, counts, remainders, pattern);
        }
        if remainders[index] != 0 {
            build(level - 2, counts, remainders, pattern);
        }
    }
}

/// Left-rotate so the pattern is unchanged when `steps` is zero.
pub fn rotate(mut pattern: Vec<bool>, steps: usize) -> Vec<bool> {
    if !pattern.is_empty() {
        let steps = steps % pattern.len();
        pattern.rotate_left(steps);
    }
    pattern
}

/// Step indexes where the pattern has a hit.
pub fn hit_steps(pattern: &[bool]) -> Vec<usize> {
    pattern
        .iter()
        .enumerate()
        .filter_map(|(index, hit)| hit.then_some(index))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{euclidean, hit_steps, rotate};

    #[test]
    fn classic_patterns() {
        assert_eq!(
            euclidean(3, 8),
            vec![false, true, false, false, true, false, false, true]
        );
        assert_eq!(
            rotate(euclidean(3, 8), 1),
            vec![true, false, false, true, false, false, true, false]
        );
        assert_eq!(
            euclidean(5, 8),
            vec![true, false, true, true, false, true, true, false]
        );
        assert_eq!(hit_steps(&rotate(euclidean(4, 16), 3)), vec![0, 4, 8, 12]);
        assert_eq!(hit_steps(&rotate(euclidean(2, 16), 3)), vec![4, 12]);
    }

    #[test]
    fn length_matches_steps() {
        for steps in 1..24 {
            for pulses in 0..=steps {
                assert_eq!(euclidean(pulses, steps).len(), steps);
            }
        }
    }
}
