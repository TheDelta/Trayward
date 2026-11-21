//! Which sessions are archived in the Claude Code extension.
//!
//! The extension keeps the ids in its VS Code global state
//! (`hiddenSessionIds`), which VS Code stores in an SQLite database:
//! `%APPDATA%\Code\User\globalStorage\state.vscdb`. It is read, read-only,
//! with the SQLite library that ships with Windows (`winsqlite3.dll`), so the
//! app needs no SQLite of its own. Archived WSL sessions are in the same file.

use serde_json::Value;
use std::collections::HashSet;
use std::ffi::{CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::core::{PCSTR, w};

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_OPEN_READONLY: c_int = 0x1;

type Open = unsafe extern "system" fn(*const c_char, *mut *mut c_void, c_int, *const c_char) -> c_int;
type Prepare =
    unsafe extern "system" fn(*mut c_void, *const c_char, c_int, *mut *mut c_void, *mut *const c_char) -> c_int;
type Step = unsafe extern "system" fn(*mut c_void) -> c_int;
type ColumnBlob = unsafe extern "system" fn(*mut c_void, c_int) -> *const c_void;
type ColumnBytes = unsafe extern "system" fn(*mut c_void, c_int) -> c_int;
type Finalize = unsafe extern "system" fn(*mut c_void) -> c_int;
type Close = unsafe extern "system" fn(*mut c_void) -> c_int;

/// The system SQLite library, unloaded on drop.
struct Sqlite {
    lib: HMODULE,
    open: Open,
    prepare: Prepare,
    step: Step,
    column_blob: ColumnBlob,
    column_bytes: ColumnBytes,
    finalize: Finalize,
    close: Close,
}

impl Sqlite {
    fn load() -> Option<Sqlite> {
        // SAFETY: loads a system DLL from System32 only; the function pointers
        // are transmuted to the documented SQLite C signatures.
        unsafe {
            let lib = LoadLibraryExW(w!("winsqlite3.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            macro_rules! sym {
                ($name:literal, $ty:ty) => {
                    match GetProcAddress(lib, PCSTR(concat!($name, "\0").as_ptr())) {
                        Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(f),
                        None => {
                            let _ = FreeLibrary(lib);
                            return None;
                        }
                    }
                };
            }
            Some(Sqlite {
                open: sym!("sqlite3_open_v2", Open),
                prepare: sym!("sqlite3_prepare_v2", Prepare),
                step: sym!("sqlite3_step", Step),
                column_blob: sym!("sqlite3_column_blob", ColumnBlob),
                column_bytes: sym!("sqlite3_column_bytes", ColumnBytes),
                finalize: sym!("sqlite3_finalize", Finalize),
                close: sym!("sqlite3_close", Close),
                lib,
            })
        }
    }

    /// The first column of the first row of `sql`, as bytes.
    fn query_one(&self, db_path: &Path, sql: &str) -> Option<Vec<u8>> {
        let path = CString::new(db_path.to_string_lossy().as_bytes()).ok()?;
        let sql = CString::new(sql).ok()?;
        // SAFETY: SQLite handles are closed/finalized before returning; the
        // column data is copied before the statement is finalized.
        unsafe {
            let mut db = std::ptr::null_mut();
            if (self.open)(path.as_ptr(), &mut db, SQLITE_OPEN_READONLY, std::ptr::null()) != SQLITE_OK {
                (self.close)(db);
                return None;
            }
            let mut stmt = std::ptr::null_mut();
            let mut result = None;
            if (self.prepare)(db, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) == SQLITE_OK
                && (self.step)(stmt) == SQLITE_ROW
            {
                let data = (self.column_blob)(stmt, 0) as *const u8;
                let len = (self.column_bytes)(stmt, 0);
                if !data.is_null() && len > 0 {
                    result = Some(std::slice::from_raw_parts(data, len as usize).to_vec());
                }
            }
            (self.finalize)(stmt);
            (self.close)(db);
            result
        }
    }
}

impl Drop for Sqlite {
    fn drop(&mut self) {
        // SAFETY: we loaded the library and no handles outlive `query_one`.
        let _ = unsafe { FreeLibrary(self.lib) };
    }
}

/// Global state databases of the installed VS Code flavours.
fn databases() -> Vec<PathBuf> {
    let Some(appdata) = std::env::var_os("APPDATA") else { return Vec::new() };
    ["Code", "Code - Insiders"]
        .iter()
        .map(|app| PathBuf::from(&appdata).join(app).join(r"User\globalStorage\state.vscdb"))
        .filter(|p| p.is_file())
        .collect()
}

/// Archived session ids from the extension's global state JSON.
fn archived_in(json: &[u8]) -> HashSet<String> {
    serde_json::from_slice::<Value>(json)
        .ok()
        .and_then(|v| v.get("hiddenSessionIds").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// Session ids archived in the Claude Code extension, or `None` if they
/// couldn't be read (then nothing should change).
pub fn archived_sessions() -> Option<HashSet<String>> {
    if crate::demo::active() {
        return Some(HashSet::new());
    }
    let dbs = databases();
    if dbs.is_empty() {
        return None;
    }
    let sqlite = Sqlite::load()?;
    let sql = "SELECT value FROM ItemTable WHERE key = 'Anthropic.claude-code' COLLATE NOCASE";
    let mut all = HashSet::new();
    let mut any = false;
    for db in dbs {
        if let Some(json) = sqlite.query_one(&db, sql) {
            any = true;
            all.extend(archived_in(&json));
        }
    }
    any.then_some(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_archived_ids() {
        let json = br#"{"hiddenSessionIds":["a","b",3],"other":1}"#;
        let ids = archived_in(json);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("a") && ids.contains("b"));
        assert!(archived_in(b"{}").is_empty());
        assert!(archived_in(b"garbage").is_empty());
    }
}
