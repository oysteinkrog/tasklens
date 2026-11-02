//! Motion tokens (UI-FND-003). Reduced motion makes every one of these instant; callers check it.

use std::time::Duration;

/// `m_fast`: hover fills of controls, check marks, the toggle thumb, the radio dot.
pub const M_FAST: Duration = Duration::from_millis(120);
/// `m_base`: theme crossfade, menu open, toast exit.
pub const M_BASE: Duration = Duration::from_millis(200);
/// `m_slow`: NEW and EXITED fade-out, dialog open.
pub const M_SLOW: Duration = Duration::from_millis(300);

/// Easing for every entrance and property change.
pub fn ease_out_cubic(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_cubic_ends_and_midpoint() {
        assert_eq!(ease_out_cubic(0.), 0.);
        assert_eq!(ease_out_cubic(1.), 1.);
        assert!((ease_out_cubic(0.5) - 0.875).abs() < 1e-6);
    }
}
