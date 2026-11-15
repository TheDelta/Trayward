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

/// Reads SVG path data into absolute segments: M, L, H, V, C, S and Z, in
/// both cases; an arc (A) becomes a line to its end point.
pub fn parse(d: &str) -> Vec<Seg> {
    // Commands and numbers. "2.427.49" is two numbers, "-1.059" starts one.
    #[derive(Clone, Copy)]
    enum Tok {
        Cmd(char),
        Num(f32),
    }
    let mut toks = Vec::new();
    let b = d.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let ch = b[i] as char;
        if ch.is_ascii_alphabetic() {
            toks.push(Tok::Cmd(ch));
            i += 1;
        } else if ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+' {
            let start = i;
            i += 1;
            let mut dot = ch == '.';
            while i < b.len() {
                let c = b[i] as char;
                if c.is_ascii_digit() {
                    i += 1;
                } else if c == '.' && !dot {
                    dot = true;
                    i += 1;
                } else if (c == 'e' || c == 'E') && i + 1 < b.len() {
                    i += 1;
                    if b[i] == b'-' || b[i] == b'+' {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if let Ok(v) = d[start..i].parse() {
                toks.push(Tok::Num(v));
            }
        } else {
            i += 1;
        }
    }

    let mut out = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut start = (0.0, 0.0);
    // The second control point of the last curve, for S.
    let mut last_ctrl: Option<(f32, f32)> = None;
    let mut cmd = 'M';
    let mut t = 0;
    let num = |t: &mut usize| -> Option<f32> {
        match toks.get(*t) {
            Some(Tok::Num(v)) => {
                *t += 1;
                Some(*v)
            }
            _ => None,
        }
    };
    while t < toks.len() {
        if let Tok::Cmd(c) = toks[t] {
            cmd = c;
            t += 1;
            if c == 'z' || c == 'Z' {
                out.push(Seg::Close);
                (x, y) = start;
                last_ctrl = None;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
        let before = t;
        match cmd.to_ascii_uppercase() {
            'M' => {
                let (Some(nx), Some(ny)) = (num(&mut t), num(&mut t)) else { break };
                (x, y) = (bx + nx, by + ny);
                start = (x, y);
                out.push(Seg::Move(x, y));
                // Further pairs after a move are lines.
                cmd = if rel { 'l' } else { 'L' };
                last_ctrl = None;
            }
            'L' => {
                let (Some(nx), Some(ny)) = (num(&mut t), num(&mut t)) else { break };
                (x, y) = (bx + nx, by + ny);
                out.push(Seg::Line(x, y));
                last_ctrl = None;
            }
            'H' => {
                let Some(nx) = num(&mut t) else { break };
                x = bx + nx;
                out.push(Seg::Line(x, y));
                last_ctrl = None;
            }
            'V' => {
                let Some(ny) = num(&mut t) else { break };
                y = by + ny;
                out.push(Seg::Line(x, y));
                last_ctrl = None;
            }
            'C' => {
                let v: Option<Vec<f32>> = (0..6).map(|_| num(&mut t)).collect();
                let Some(v) = v else { break };
                let p = [bx + v[0], by + v[1], bx + v[2], by + v[3], bx + v[4], by + v[5]];
                out.push(Seg::Cubic(p[0], p[1], p[2], p[3], p[4], p[5]));
                last_ctrl = Some((p[2], p[3]));
                (x, y) = (p[4], p[5]);
            }
            'S' => {
                let v: Option<Vec<f32>> = (0..4).map(|_| num(&mut t)).collect();
                let Some(v) = v else { break };
                // The first control point mirrors the last one.
                let (c1x, c1y) = last_ctrl.map_or((x, y), |(cx, cy)| (2.0 * x - cx, 2.0 * y - cy));
                let p = [bx + v[0], by + v[1], bx + v[2], by + v[3]];
                out.push(Seg::Cubic(c1x, c1y, p[0], p[1], p[2], p[3]));
                last_ctrl = Some((p[0], p[1]));
                (x, y) = (p[2], p[3]);
            }
            'A' => {
                let v: Option<Vec<f32>> = (0..7).map(|_| num(&mut t)).collect();
                let Some(v) = v else { break };
                (x, y) = (bx + v[5], by + v[6]);
                out.push(Seg::Line(x, y));
                last_ctrl = None;
            }
            _ => t += 1,
        }
        // Nothing read: stop rather than loop forever.
        if t == before {
            break;
        }
    }
    out
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
