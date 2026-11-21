//! Clickable areas, as the flyout, the settings and the statistics window
//! collect them while laying out: a rectangle and what a click on it does.
//! Hovering is by index: the area at the mouse in the last layout is drawn
//! highlighted in the next one, so layout and drawing must push the same
//! areas in the same order.

/// One clickable area (in DIPs). `tag` is what the window needs to know
/// besides: the flyout its highlight group, the statistics window whether the
/// area scrolls with the page.
#[derive(Clone, Debug)]
pub struct Hit<A, T = ()> {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub action: A,
    pub tag: T,
}

impl<A, T> Hit<A, T> {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// The area at (`x`, `y`): of several, the one pushed first (buttons are
/// pushed before the row they sit on).
pub fn find<A, T>(hits: &[Hit<A, T>], x: f32, y: f32) -> Option<usize> {
    hits.iter().position(|h| h.contains(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_first_area_at_a_point() {
        let hit = |x, w, action| Hit { x, y: 0.0, w, h: 10.0, action, tag: () };
        let hits = [hit(80.0, 20.0, "button"), hit(0.0, 100.0, "row")];
        assert_eq!(find(&hits, 85.0, 5.0), Some(0));
        assert_eq!(find(&hits, 10.0, 5.0), Some(1));
        // The right and bottom edges are outside.
        assert_eq!(find(&hits, 100.0, 5.0), None);
        assert_eq!(find(&hits, 10.0, 10.0), None);
        assert_eq!(find::<&str, ()>(&[], 1.0, 1.0), None);
    }
}
