// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

const KG_PER_LB: f64 = 0.453_592_37;

pub fn lb_to_kg(lb: f64) -> f64 {
    lb * KG_PER_LB
}

pub fn kg_to_lb(kg: f64) -> f64 {
    kg / KG_PER_LB
}

/// Rounds to the nearest multiple of `increment`, half-up (e.g. 112.5 @ 5.0 -> 115.0).
pub fn round_to_nearest(value: f64, increment: f64) -> f64 {
    (value / increment).round() * increment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_half_up_to_nearest_five() {
        assert_eq!(round_to_nearest(112.5, 5.0), 115.0);
        assert_eq!(round_to_nearest(113.75, 5.0), 115.0);
        assert_eq!(round_to_nearest(110.0, 5.0), 110.0);
    }

    #[test]
    fn rounds_to_nearest_two_point_five() {
        // 113.75 is exactly halfway between 112.5 and 115.0; half-up -> 115.0.
        assert_eq!(round_to_nearest(113.75, 2.5), 115.0);
        assert_eq!(round_to_nearest(111.5, 2.5), 112.5);
    }

    #[test]
    fn lb_kg_roundtrip_is_stable() {
        let lb = 175.0;
        let kg = lb_to_kg(lb);
        assert!((kg_to_lb(kg) - lb).abs() < 1e-9);
    }
}
