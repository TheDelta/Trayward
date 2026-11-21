//! The GitLab and GitHub logos, drawn as vector paths (no image files), for
//! the merge and pull request bubbles. Each says which service a request is on.

use crate::forge::Forge;
use crate::gfx::{Canvas, Seg};
use crate::theme::{Color, Palette};

/// The GitHub mark (github.com/logos), 98 × 96.
const GITHUB: &str = "M48.854 0C21.839 0 0 22 0 49.217c0 21.756 13.993 40.172 33.405 46.69 2.427.49 3.316-1.059 \
3.316-2.362 0-1.141-.08-5.052-.08-9.127-13.59 2.934-16.42-5.867-16.42-5.867-2.184-5.704-5.42-7.17-5.42-7.17-4.448-3.015\
.324-3.015.324-3.015 4.934.326 7.523 5.052 7.523 5.052 4.367 7.496 11.404 5.378 14.235 4.074.404-3.178 1.699-5.378 \
3.074-6.6-10.839-1.141-22.243-5.378-22.243-24.283 0-5.378 1.94-9.778 5.014-13.2-.485-1.222-2.184-6.275.486-13.038 0 0 \
4.125-1.304 13.426 5.052a46.97 46.97 0 0 1 12.214-1.63c4.125 0 8.33.571 12.213 1.63 9.302-6.356 13.427-5.052 \
13.427-5.052 2.67 6.763.97 11.816.485 13.038 3.155 3.422 5.015 7.822 5.015 13.2 0 18.905-11.404 23.06-22.324 24.283 \
1.78 1.548 3.316 4.481 3.316 9.126 0 6.6-.08 11.897-.08 13.526 0 1.304.89 2.853 3.316 2.364 19.412-6.52 33.405-24.935 \
33.405-46.691C97.707 22 75.788 0 48.854 0z";
const GITHUB_SIZE: (f32, f32) = (98.0, 96.0);

/// GitLab's tanuki in its three oranges, 210 × 194: the face, the cheeks,
/// the jaw corners and the ears.
const GITLAB: [(&str, u32); 7] = [
    ("M105 194L143.7 74.7H66.3z", 0xE24329),
    ("M105 194L66.3 74.7H12.3z", 0xFC6D26),
    ("M12.3 74.7L.5 110.9c-1.1 3.3.1 6.9 2.9 8.9L105 194z", 0xFCA326),
    ("M12.3 74.7H66.3L43.1 3.1c-1.2-3.7-6.4-3.7-7.6 0z", 0xE24329),
    ("M105 194L143.7 74.7H197.7z", 0xFC6D26),
    ("M197.7 74.7L209.5 110.9c1.1 3.3-.1 6.9-2.9 8.9L105 194z", 0xFCA326),
    ("M197.7 74.7H143.7L166.9 3.1c1.2-3.7 6.4-3.7 7.6 0z", 0xE24329),
];
const GITLAB_SIZE: (f32, f32) = (210.0, 194.0);

fn rgb(hex: u32) -> Color {
    let c = |shift: u32| ((hex >> shift) & 0xFF) as f32 / 255.0;
    Color { r: c(16), g: c(8), b: c(0), a: 1.0 }
}

/// Draws a service's logo in a `size` × `size` square at (x, y). GitHub's
/// mark takes the text colour, as GitHub asks; GitLab's keeps its colours.
pub fn draw(c: &Canvas, forge: Forge, x: f32, y: f32, size: f32, pal: &Palette) {
    let fit = |(w, h): (f32, f32)| {
        let scale = size / w.max(h);
        (scale, x + (size - w * scale) / 2.0, y + (size - h * scale) / 2.0)
    };
    match forge {
        Forge::GitHub => {
            let (scale, ox, oy) = fit(GITHUB_SIZE);
            c.fill_path(&parse(GITHUB), ox, oy, scale, pal.text);
        }
        Forge::GitLab => {
            let (scale, ox, oy) = fit(GITLAB_SIZE);
            for (d, color) in GITLAB {
                c.fill_path(&parse(d), ox, oy, scale, rgb(color));
            }
        }
    }
}

/// A piece of SVG path data: a command or a number.
#[derive(Clone, Copy)]
enum Tok {
    Cmd(char),
    Num(f32),
}

/// Where the number that starts at `start` ends: digits, one dot, an exponent with its sign.
fn number_end(b: &[u8], start: usize) -> usize {
    let mut dot = b.get(start) == Some(&b'.');
    let mut i = start + 1;
    while let Some(&c) = b.get(i) {
        match c {
            b'0'..=b'9' => i += 1,
            b'.' if !dot => {
                dot = true;
                i += 1;
            }
            b'e' | b'E' if i + 1 < b.len() => i += if matches!(b.get(i + 1), Some(b'-' | b'+')) { 2 } else { 1 },
            _ => break,
        }
    }
    i
}

/// Commands and numbers. "2.427.49" is two numbers, "-1.059" starts one.
fn tokens(d: &str) -> Vec<Tok> {
    let b = d.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    while let Some(&byte) = b.get(i) {
        let ch = byte as char;
        if ch.is_ascii_alphabetic() {
            toks.push(Tok::Cmd(ch));
            i += 1;
        } else if ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+') {
            let end = number_end(b, i);
            if let Some(v) = d.get(i..end).and_then(|number| number.parse().ok()) {
                toks.push(Tok::Num(v));
            }
            i = end;
        } else {
            i += 1;
        }
    }
    toks
}

/// How many numbers a command (in upper case) takes; None for one this reader doesn't know.
fn numbers_of(command: char) -> Option<usize> {
    match command {
        'H' | 'V' => Some(1),
        'M' | 'L' => Some(2),
        'S' => Some(4),
        'C' => Some(6),
        'A' => Some(7),
        _ => None,
    }
}

/// The pen that draws a path: where it is, where its figure started, and the
/// second control point of the last curve (for S).
#[derive(Default)]
struct Pen {
    x: f32,
    y: f32,
    start: (f32, f32),
    last_ctrl: Option<(f32, f32)>,
    out: Vec<Seg>,
}

impl Pen {
    fn line_to(&mut self, x: f32, y: f32) {
        (self.x, self.y) = (x, y);
        self.out.push(Seg::Line(x, y));
        self.last_ctrl = None;
    }

    fn close(&mut self) {
        self.out.push(Seg::Close);
        (self.x, self.y) = self.start;
        self.last_ctrl = None;
    }

    /// Draws `command` (in upper case) with its numbers `n`; relative ones count from `(bx, by)`.
    fn draw(&mut self, command: char, (bx, by): (f32, f32), n: &[f32]) {
        match (command, n) {
            ('M', &[x, y]) => {
                (self.x, self.y) = (bx + x, by + y);
                self.start = (self.x, self.y);
                self.out.push(Seg::Move(self.x, self.y));
                self.last_ctrl = None;
            }
            ('L', &[x, y]) => self.line_to(bx + x, by + y),
            ('H', &[x]) => self.line_to(bx + x, self.y),
            ('V', &[y]) => self.line_to(self.x, by + y),
            ('C', &[x1, y1, x2, y2, x, y]) => {
                let (c2, end) = ((bx + x2, by + y2), (bx + x, by + y));
                self.out.push(Seg::Cubic(bx + x1, by + y1, c2.0, c2.1, end.0, end.1));
                self.last_ctrl = Some(c2);
                (self.x, self.y) = end;
            }
            ('S', &[x2, y2, x, y]) => {
                // The first control point mirrors the last one.
                let c1 = self.last_ctrl.map_or((self.x, self.y), |(cx, cy)| (2.0 * self.x - cx, 2.0 * self.y - cy));
                let (c2, end) = ((bx + x2, by + y2), (bx + x, by + y));
                self.out.push(Seg::Cubic(c1.0, c1.1, c2.0, c2.1, end.0, end.1));
                self.last_ctrl = Some(c2);
                (self.x, self.y) = end;
            }
            // An arc becomes a line to its end point.
            ('A', &[.., x, y]) => self.line_to(bx + x, by + y),
            _ => {}
        }
    }
}

/// Reads SVG path data into absolute segments: M, L, H, V, C, S and Z, in
/// both cases; an arc (A) becomes a line to its end point.
pub fn parse(d: &str) -> Vec<Seg> {
    let toks = tokens(d);
    let mut pen = Pen::default();
    let mut cmd = 'M';
    let mut t = 0;
    while let Some(tok) = toks.get(t) {
        if let Tok::Cmd(c) = *tok {
            cmd = c;
            t += 1;
            if c.eq_ignore_ascii_case(&'z') {
                pen.close();
                continue;
            }
        }
        let (rel, command) = (cmd.is_ascii_lowercase(), cmd.to_ascii_uppercase());
        // A command this reader doesn't know: its numbers are passed over, one by one.
        let Some(count) = numbers_of(command) else {
            t += 1;
            continue;
        };
        // All of its numbers, or the path ends here.
        let numbers: Vec<f32> = toks
            .iter()
            .skip(t)
            .take(count)
            .map_while(|tok| match tok {
                Tok::Num(v) => Some(*v),
                Tok::Cmd(_) => None,
            })
            .collect();
        if numbers.len() < count {
            break;
        }
        t += count;
        pen.draw(command, if rel { (pen.x, pen.y) } else { (0.0, 0.0) }, &numbers);
        // Further pairs after a move are lines.
        if command == 'M' {
            cmd = if rel { 'l' } else { 'L' };
        }
    }
    pen.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(segs: &[Seg]) -> (f32, f32, f32, f32) {
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut add = |x: f32, y: f32| b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
        for s in segs {
            match *s {
                Seg::Move(x, y) | Seg::Line(x, y) => add(x, y),
                Seg::Cubic(_, _, _, _, x, y) => add(x, y),
                Seg::Close => {}
            }
        }
        b
    }

    #[test]
    fn reads_svg_paths() {
        let segs = parse("M10 20l5-5h-.5V3.5c1 1 2 2 3 3s4 4 5 5z");
        assert_eq!(segs[0], Seg::Move(10.0, 20.0));
        assert_eq!(segs[1], Seg::Line(15.0, 15.0));
        assert_eq!(segs[2], Seg::Line(14.5, 15.0));
        assert_eq!(segs[3], Seg::Line(14.5, 3.5));
        assert_eq!(segs[4], Seg::Cubic(15.5, 4.5, 16.5, 5.5, 17.5, 6.5));
        // S mirrors the last control point (16.5, 5.5) around (17.5, 6.5).
        assert_eq!(segs[5], Seg::Cubic(18.5, 7.5, 21.5, 10.5, 22.5, 11.5));
        assert_eq!(segs[6], Seg::Close);
        // Pairs after a move are lines; junk ends the path instead of hanging.
        assert_eq!(parse("M0 0 1 1"), [Seg::Move(0.0, 0.0), Seg::Line(1.0, 1.0)]);
        assert_eq!(parse("M0 0 L"), [Seg::Move(0.0, 0.0)]);
    }

    #[test]
    fn logos_fill_their_boxes() {
        let (x0, y0, x1, y1) = bounds(&parse(GITHUB));
        assert!(x0.abs() < 0.5 && y0.abs() < 0.5, "{x0} {y0}");
        assert!((x1 - GITHUB_SIZE.0).abs() < 1.0 && (y1 - GITHUB_SIZE.1).abs() < 1.5, "{x1} {y1}");
        // The mark is one closed outline.
        let github = parse(GITHUB);
        assert_eq!(github.iter().filter(|s| matches!(s, Seg::Move(..))).count(), 1);
        assert_eq!(github.last(), Some(&Seg::Close));
        let all: Vec<Seg> = GITLAB.iter().flat_map(|(d, _)| parse(d)).collect();
        let (x0, y0, x1, y1) = bounds(&all);
        assert!(x0 < 1.0 && y0 < 4.0 && x1 > 208.0 && (y1 - 194.0).abs() < 0.5, "{x0} {y0} {x1} {y1}");
    }
}
