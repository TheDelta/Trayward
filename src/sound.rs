//! Sounds for "a session needs you" and "a session is done".
//!
//! The built-in sounds are synthesised here, so there are no audio files to
//! ship: soft bell and marimba-like tones made of a few sine partials, each
//! with a short fade-in (no click) and a natural decay (no ringing). They are
//! quiet and short on purpose. "Done" sounds resolve (they land on a home
//! note); "needs you" sounds stay open, like a question or a knock.

use crate::settings::{SoundChoice, Volume};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_MEMORY, SND_NODEFAULT};
use windows::Win32::UI::Shell::{QUNS_ACCEPTS_NOTIFICATIONS, QUNS_APP, SHQueryUserNotificationState};
use windows::core::{HSTRING, PCWSTR};

/// What a sound announces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    NeedsYou,
    Done,
}

/// Names of the built-in sounds, numbered 1 to 3, for each kind.
pub fn names(kind: Kind) -> [&'static str; 3] {
    match kind {
        Kind::NeedsYou => ["Knock", "Ping", "Nudge"],
        Kind::Done => ["Chime", "Marimba", "Arpeggio"],
    }
}

/// One line about each built-in sound, numbered 1 to 3.
pub fn descriptions(kind: Kind) -> [&'static str; 3] {
    match kind {
        Kind::NeedsYou => ["Two soft taps on wood", "A single gentle bell", "A small step up, like \"hm?\""],
        Kind::Done => ["A bright rising fifth", "Warm marimba, landing home", "A short major chord"],
    }
}

const RATE: u32 = 44_100;

/// A struck note: partials (frequency ratio, level, decay per second).
struct Timbre(&'static [(f32, f32, f32)]);

/// Glass-like bell: clear fundamental, a soft octave, a faint shimmer.
const BELL: Timbre = Timbre(&[(1.0, 1.0, 3.2), (2.0, 0.28, 5.5), (3.01, 0.08, 9.0)]);
/// Marimba: warm fundamental and the characteristic fourth partial, dying quickly.
const MARIMBA: Timbre = Timbre(&[(1.0, 1.0, 5.0), (3.93, 0.22, 16.0), (9.2, 0.04, 30.0)]);
/// Wood block: short and dry, like knocking on a door.
const WOOD: Timbre = Timbre(&[(1.0, 1.0, 26.0), (2.76, 0.45, 40.0), (5.4, 0.15, 60.0)]);

/// A note in a sound: start (s), frequency (Hz), level, timbre.
struct Note(f32, f32, f32, &'static Timbre);

fn notes(kind: Kind, n: u8) -> Vec<Note> {
    match (kind, n) {
        // Two soft taps on wood.
        (Kind::NeedsYou, 1) => vec![Note(0.0, 330.0, 1.0, &WOOD), Note(0.17, 330.0, 0.85, &WOOD)],
        // One gentle bell and a quiet echo of it.
        (Kind::NeedsYou, 2) => vec![Note(0.0, 1174.7, 0.8, &BELL), Note(0.22, 1174.7, 0.25, &BELL)],
        // A small step up that doesn't resolve, like "hm?" (A5 to B5).
        (Kind::NeedsYou, _) => vec![Note(0.0, 880.0, 0.75, &BELL), Note(0.14, 987.8, 0.75, &BELL)],
        // A rising fifth: bright and complete (C5 to G5).
        (Kind::Done, 1) => vec![Note(0.0, 523.3, 0.8, &BELL), Note(0.12, 784.0, 0.8, &BELL)],
        // Marimba resolving from the fifth to the home note (G4 to C5).
        (Kind::Done, 2) => vec![Note(0.0, 392.0, 0.9, &MARIMBA), Note(0.13, 523.3, 1.0, &MARIMBA)],
        // A quick major arpeggio (C5, E5, G5).
        (Kind::Done, _) => {
            vec![Note(0.0, 523.3, 0.7, &BELL), Note(0.09, 659.3, 0.7, &BELL), Note(0.18, 784.0, 0.8, &BELL)]
        }
    }
}

fn peak(volume: Volume) -> f32 {
    match volume {
        Volume::Quiet => 0.10,
        Volume::Medium => 0.20,
        Volume::Loud => 0.34,
    }
}

/// Renders a built-in sound as 16-bit mono PCM samples.
fn synthesize(kind: Kind, n: u8, volume: Volume) -> Vec<i16> {
    const ATTACK: f32 = 0.006;
    const LENGTH: f32 = 1.2;
    let notes = notes(kind, n);
    let total = (LENGTH * RATE as f32) as usize;
    let mut mix = vec![0f32; total];
    for Note(start, freq, level, timbre) in &notes {
        let first = (start * RATE as f32) as usize;
        for (i, out) in mix.iter_mut().enumerate().skip(first) {
            let t = (i - first) as f32 / RATE as f32;
            // Raised-cosine fade-in avoids a click at the start.
            let attack = if t < ATTACK { 0.5 - 0.5 * (std::f32::consts::PI * t / ATTACK).cos() } else { 1.0 };
            let mut v = 0.0;
            for &(ratio, amp, decay) in timbre.0 {
                v += amp * (-decay * t).exp() * (std::f32::consts::TAU * freq * ratio * t).sin();
            }
            *out += level * attack * v;
        }
    }
    // Fade the tail to silence and scale to the chosen volume.
    let tail = (0.08 * RATE as f32) as usize;
    for i in 0..tail {
        mix[total - 1 - i] *= i as f32 / tail as f32;
    }
    let max = mix.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    let scale = peak(volume) / max;
    mix.iter().map(|v| (v * scale * i16::MAX as f32) as i16).collect()
}

/// Wraps samples in a WAV (RIFF) container.
fn wav(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Rendered sounds. Entries are never removed: Windows plays them
/// asynchronously straight from this memory.
/// Rendered WAV data per kind, sound number and volume.
type Cache = Mutex<HashMap<(Kind, u8, Volume), &'static [u8]>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn builtin(kind: Kind, n: u8, volume: Volume) -> &'static [u8] {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.entry((kind, n, volume)).or_insert_with(|| Box::leak(wav(&synthesize(kind, n, volume)).into_boxed_slice()))
}

/// Whether sounds and bubbles should wait: the user is presenting, in a
/// full-screen app, or has Windows' "Do not disturb" on.
pub fn quiet() -> bool {
    // SAFETY: plain query.
    let busy = match unsafe { SHQueryUserNotificationState() } {
        Ok(state) => state != QUNS_ACCEPTS_NOTIFICATIONS && state != QUNS_APP,
        Err(_) => false,
    };
    busy || do_not_disturb()
}

/// Windows' "Do not disturb" (Focus assist on Windows 10): the active quiet
/// hours profile, 0 when off. There's no documented API for it; this reads the
/// notification state Windows publishes for the shell (WNF), as other apps do.
/// Unknown (the call fails): not quiet.
fn do_not_disturb() -> bool {
    quiet_hours_profile().is_some_and(|p| p != 0)
}

/// The active quiet hours profile: 0 off, 1 priority only, 2 alarms only.
fn quiet_hours_profile() -> Option<u32> {
    windows::core::link!("ntdll.dll" "system" fn NtQueryWnfStateData(
        state_name: *const u64,
        type_id: *const core::ffi::c_void,
        explicit_scope: *const core::ffi::c_void,
        change_stamp: *mut u32,
        buffer: *mut core::ffi::c_void,
        buffer_size: *mut u32
    ) -> i32);
    const WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED: u64 = 0x0D83_063E_A3BF_1C75;
    let (mut stamp, mut profile, mut size) = (0u32, 0u32, 4u32);
    // SAFETY: the buffer is a u32 of the size passed; the other pointers are
    // optional and null.
    let status = unsafe {
        NtQueryWnfStateData(
            &WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED,
            std::ptr::null(),
            std::ptr::null(),
            &mut stamp,
            (&mut profile as *mut u32).cast(),
            &mut size,
        )
    };
    (status >= 0 && size >= 4).then_some(profile)
}

/// Plays `choice` for `kind`. `preview` plays even when the user is busy.
pub fn play(kind: Kind, choice: &SoundChoice, volume: Volume, preview: bool) {
    if !preview && quiet() {
        return;
    }
    // SAFETY: the WAV memory is 'static; file names are NUL-terminated by HSTRING.
    unsafe {
        let _ = match choice {
            SoundChoice::Off => return,
            SoundChoice::Builtin(n) => {
                let data = builtin(kind, *n, volume);
                PlaySoundW(PCWSTR(data.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT)
            }
            SoundChoice::File(path) => {
                // On a thread: the file is opened and read before the call
                // returns, and it may be on a slow drive.
                let path = HSTRING::from(path.as_str());
                std::thread::spawn(move || {
                    let _ = PlaySoundW(&path, None, SND_FILENAME | SND_ASYNC | SND_NODEFAULT);
                });
                return;
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_short_quiet_and_click_free() {
        for kind in [Kind::NeedsYou, Kind::Done] {
            for n in 1..=3 {
                let s = synthesize(kind, n, Volume::Medium);
                assert_eq!(s.len(), (1.2 * RATE as f32) as usize);
                let max = s.iter().map(|v| i32::from(*v).abs()).max().unwrap();
                // Peak is the medium level, never clipping.
                assert!((f32::from(max as i16) / i16::MAX as f32 - 0.20).abs() < 0.01, "{kind:?} {n}");
                // Starts and ends at silence.
                assert_eq!(s[0], 0);
                assert!(i32::from(*s.last().unwrap()).abs() < 50);
            }
        }
    }

    #[test]
    fn writes_a_valid_wav_header() {
        let w = wav(&[0, 1, -1]);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(&w[36..40], b"data");
        assert_eq!(w.len(), 44 + 6);
    }

    /// Windows 11 publishes the state (Windows Server may not: CI skips it).
    #[test]
    #[ignore]
    fn reads_do_not_disturb() {
        assert!(quiet_hours_profile().is_some());
    }

    #[test]
    fn quiet_is_quieter_than_loud() {
        let max = |v| synthesize(Kind::Done, 1, v).iter().map(|s| i32::from(*s).abs()).max().unwrap();
        assert!(max(Volume::Quiet) < max(Volume::Medium));
        assert!(max(Volume::Medium) < max(Volume::Loud));
    }
}
