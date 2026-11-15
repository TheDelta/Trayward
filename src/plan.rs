//! How the subscription plan's name looks. Claude's plans are Max and Ultra:
//! Max in a two-colour gradient, Ultra on a badge in a richer gradient with a
//! glow behind its star and, while the user looks at it (the mouse on the
//! widget, the flyout open), a shine sweeping across.

use crate::gfx::{Canvas, Text};
use crate::theme::{Color, Palette};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Max,
    Ultra,
}

/// The tier of a plan name as the usage service gives it ("Max 20x", "Ultra").
/// Names Trayward doesn't know (older plans, Team, Enterprise) look like Max.
pub fn tier(plan: &str) -> Tier {
    if plan.trim().to_ascii_lowercase().starts_with("ultra") { Tier::Ultra } else { Tier::Max }
}

/// The text shown for a plan.
pub fn label(plan: &str) -> String {
    format!("✦ {}", plan.trim())
}

/// How far the Ultra badge reaches beyond its text on each side (DIPs).
pub const BADGE_PAD: f32 = 5.0;

/// Extra width a plan's look needs beyond its text, on each side.
pub fn margin(plan: &str) -> f32 {
    if tier(plan) == Tier::Ultra { BADGE_PAD } else { 0.0 }
}

/// One shine sweep, then a pause (seconds).
const SHINE_SWEEP: f32 = 1.4;
const SHINE_EVERY: f32 = 4.0;

/// Where the shine is at animation time `clock` (0 to 1 across the text), or
/// None between sweeps. Rounded, so frames only redraw when it moves visibly.
pub fn shine(clock: f32) -> Option<f32> {
    let t = clock.rem_euclid(SHINE_EVERY);
    // From a bit before the text to a bit after it, so it enters and leaves softly.
    (t < SHINE_SWEEP).then(|| ((t / SHINE_SWEEP * 1.5 - 0.25) * 50.0).round() / 50.0)
}

/// Draws the plan's text `t` (made from [`label`]) at (x, y).
/// `fade` dims it (old numbers); `shine` is from [`shine`].
#[allow(clippy::too_many_arguments)] // position, look and state of one label
pub fn draw(c: &Canvas, t: &Text, x: f32, y: f32, plan: &str, pal: &Palette, fade: f32, shine: Option<f32>) {
    match tier(plan) {
        Tier::Max => c.text_gradient(t, x, y, pal.plan_from.with_alpha(fade), pal.plan_to.with_alpha(fade)),
        Tier::Ultra => {
            let base: Vec<(f32, Color)> = pal
                .ultra
                .iter()
                .enumerate()
                .map(|(i, c)| (i as f32 / (pal.ultra.len() - 1) as f32, c.with_alpha(fade)))
                .collect();
            // A badge: a tinted pill with an outline, a little wider than the text.
            let (bx, by, bw, bh) = (x - BADGE_PAD, y - 1.0, t.width + 2.0 * BADGE_PAD, t.height + 2.0);
            c.fill_round_rect(bx, by, bw, bh, bh / 2.0, pal.ultra[2].with_alpha(0.14 * fade));
            c.stroke_round_rect(
                bx + 0.5,
                by + 0.5,
                bw - 1.0,
                bh - 1.0,
                (bh - 1.0) / 2.0,
                1.0,
                pal.ultra[1].with_alpha(0.45 * fade),
            );
            // A soft glow behind the star.
            let star = t.height * 0.45;
            c.glow(x + star, y + t.height / 2.0, t.height * 0.9, pal.ultra[0].with_alpha(0.30 * fade));
            let stops = match shine {
                Some(s) => with_shine(&base, s, pal.shine.with_alpha(fade)),
                None => base,
            };
            c.text_stops(t, x, y, &stops);
        }
    }
}

/// The gradient `stops` with a narrow bright band around `at`.
fn with_shine(stops: &[(f32, Color)], at: f32, light: Color) -> Vec<(f32, Color)> {
    const HALF: f32 = 0.12;
    let mut out: Vec<(f32, Color)> = stops.iter().copied().filter(|(p, _)| (*p - at).abs() >= HALF).collect();
    for p in [at - HALF, at + HALF] {
        if (0.0..=1.0).contains(&p) {
            out.push((p, sample(stops, p)));
        }
    }
    if (0.0..=1.0).contains(&at) {
        out.push((at, light));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// The gradient's colour at `p` (0 to 1).
fn sample(stops: &[(f32, Color)], p: f32) -> Color {
    let Some(first) = stops.first() else { return Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 } };
    let mut prev = *first;
    for &(q, c) in stops {
        if q >= p {
            let span = (q - prev.0).max(f32::EPSILON);
            let f = ((p - prev.0) / span).clamp(0.0, 1.0);
            let mix = |a: f32, b: f32| a + (b - a) * f;
            return Color {
                r: mix(prev.1.r, c.r),
                g: mix(prev.1.g, c.g),
                b: mix(prev.1.b, c.b),
                a: mix(prev.1.a, c.a),
            };
        }
        prev = (q, c);
    }
    prev.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_tiers() {
        assert_eq!(tier("Ultra"), Tier::Ultra);
        assert_eq!(tier("Max 20x"), Tier::Max);
        assert_eq!(tier("Max"), Tier::Max);
        assert_eq!(tier("Team"), Tier::Max);
        assert_eq!(label("Ultra"), "✦ Ultra");
    }

    #[test]
    fn shines_now_and_then() {
        assert!(shine(0.5).is_some());
        assert!(shine(SHINE_SWEEP + 0.5).is_none());
        assert!(shine(SHINE_EVERY + 0.5).is_some());
    }

    #[test]
    fn the_shine_keeps_stops_in_order() {
        let c = |r| Color { r, g: 0.0, b: 0.0, a: 1.0 };
        let stops = [(0.0, c(0.0)), (0.5, c(0.5)), (1.0, c(1.0))];
        let out = with_shine(&stops, 0.5, c(9.0));
        assert!(out.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(out.iter().any(|(p, col)| *p == 0.5 && col.r == 9.0));
        assert!((sample(&stops, 0.25).r - 0.25).abs() < 1e-6);
    }
}
