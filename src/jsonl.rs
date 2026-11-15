//! Append-only JSON-lines files (Claude Code's transcripts, Copilot's
//! events), read as they grow: only what was added since the last time, and
//! only whole lines. One reader for all of them.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// How much is read at once: such files can be hundreds of megabytes.
const READ_CHUNK: usize = 4 << 20;

/// Where reading a file has got to.
#[derive(Default)]
pub struct Tail {
    /// Bytes read so far.
    offset: u64,
    /// The start of a line that isn't complete yet.
    carry: Vec<u8>,
}

/// How a read went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Everything there is has been read.
    UpToDate,
    /// The limit was reached; more is left for the next time.
    More,
    /// The file is shorter than what was read: it was replaced. Nothing was
    /// read, and the next read starts at its beginning: forget what the old
    /// file said, then read again.
    Replaced,
    /// It couldn't be opened or read; what was read before stays.
    Unreadable,
}

impl Tail {
    /// Reads what was added to `path`, at most `limit` bytes of it, and hands
    /// each whole line to `each` (a line still being written waits for the
    /// next read; invalid UTF-8 is replaced, not dropped).
    pub fn read(&mut self, path: &Path, limit: Option<u64>, mut each: impl FnMut(&str)) -> Outcome {
        let Ok(mut file) = std::fs::File::open(path) else { return Outcome::Unreadable };
        let Ok(len) = file.metadata().map(|m| m.len()) else { return Outcome::Unreadable };
        if len < self.offset {
            *self = Tail::default();
            return Outcome::Replaced;
        }
        let until = limit.map_or(len, |l| len.min(self.offset.saturating_add(l)));
        if self.offset < until {
            if file.seek(SeekFrom::Start(self.offset)).is_err() {
                return Outcome::Unreadable;
            }
            let mut chunk = vec![0u8; READ_CHUNK.min((until - self.offset) as usize)];
            while self.offset < until {
                let want = chunk.len().min((until - self.offset) as usize);
                let n = match file.read(&mut chunk[..want]) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => return Outcome::Unreadable,
                };
                self.offset += n as u64;
                self.carry.extend_from_slice(&chunk[..n]);
                let complete = self.carry.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
                for line in self.carry[..complete].split(|&b| b == b'\n') {
                    let line = line.strip_suffix(b"\r").unwrap_or(line);
                    if !line.is_empty() {
                        each(&String::from_utf8_lossy(line));
                    }
                }
                self.carry.drain(..complete);
            }
            self.carry.shrink_to_fit();
        }
        if self.offset >= len { Outcome::UpToDate } else { Outcome::More }
    }

    /// Bytes of whole lines read so far.
    #[cfg(test)]
    pub fn lines_end(&self) -> u64 {
        self.offset - self.carry.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_whole_lines_as_the_file_grows() {
        let path = std::env::temp_dir().join(format!("trayward-jsonl-test-{}.jsonl", std::process::id()));
        std::fs::write(&path, "one\ntwo\r\n\n3 is").unwrap();
        let mut tail = Tail::default();
        let mut lines = Vec::new();
        assert_eq!(tail.read(&path, None, |l| lines.push(l.to_string())), Outcome::UpToDate);
        // The line still being written waits; empty lines and line ends are left out.
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(tail.lines_end(), 10);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 14);
        // Nothing new: nothing read.
        assert_eq!(tail.read(&path, None, |l| lines.push(l.to_string())), Outcome::UpToDate);
        assert_eq!(lines.len(), 2);

        // More is added, with a character cut in two at a read's limit ("ü" is two bytes).
        std::fs::File::options().append(true).open(&path).unwrap().write_all(" 3\nfür\nx\n".as_bytes()).unwrap();
        assert_eq!(tail.read(&path, Some(5), |l| lines.push(l.to_string())), Outcome::More);
        assert_eq!(lines, ["one", "two", "3 is 3"]);
        assert_eq!(tail.read(&path, None, |l| lines.push(l.to_string())), Outcome::UpToDate);
        assert_eq!(lines, ["one", "two", "3 is 3", "für", "x"]);

        // Replaced by a shorter file: said so, then read from the start.
        std::fs::write(&path, "new\n").unwrap();
        assert_eq!(tail.read(&path, None, |_| panic!("nothing is read")), Outcome::Replaced);
        lines.clear();
        assert_eq!(tail.read(&path, None, |l| lines.push(l.to_string())), Outcome::UpToDate);
        assert_eq!(lines, ["new"]);

        // Gone: unreadable, and what was read stays.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(tail.read(&path, None, |_| {}), Outcome::Unreadable);
        assert_eq!(tail.lines_end(), 4);
    }
}
