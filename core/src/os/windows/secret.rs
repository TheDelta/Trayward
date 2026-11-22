//! Secrets in Windows Credential Manager: generic credentials of the user, which
//! Windows keeps encrypted with the user's login (DPAPI). Only Trayward's own
//! entries are read, written or listed (their names start with `trayward:`); the
//! user sees them under "Windows Credentials" and can remove them there too.

use windows::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredEnumerateW, CredFree, CredReadW,
    CredWriteW,
};
use windows::core::{HSTRING, PWSTR};

/// What's kept under `target`, if anything (and it's text).
pub fn read(target: &str) -> Option<String> {
    let name = HSTRING::from(target);
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: CredReadW hands out a block Windows allocated; its blob is read
    // within its size, and the block is freed once.
    unsafe {
        CredReadW(&name, CRED_TYPE_GENERIC, None, &mut cred).ok()?;
        let secret = cred.as_ref().and_then(|c| {
            if c.CredentialBlob.is_null() {
                return None;
            }
            decode(std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize))
        });
        CredFree(cred.cast_const().cast());
        secret
    }
}

/// A secret's text: UTF-8 as Trayward writes it, or UTF-16 as Credential
/// Manager's own Edit dialog keeps a password.
fn decode(blob: &[u8]) -> Option<String> {
    if blob.len() % 2 == 0 && blob.contains(&0) {
        let units: Vec<u16> = blob.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
        return String::from_utf16(&units).ok().map(|s| s.trim_end_matches('\0').to_string());
    }
    String::from_utf8(blob.to_vec()).ok()
}

/// Keeps `secret` under `target` (replacing what was there), with `user` as the
/// name Credential Manager shows for it.
pub fn write(target: &str, user: &str, secret: &str) -> Result<(), String> {
    let mut name: Vec<u16> = target.encode_utf16().chain([0]).collect();
    let mut user: Vec<u16> = user.encode_utf16().chain([0]).collect();
    let mut blob = secret.as_bytes().to_vec();
    let size = u32::try_from(blob.len()).map_err(|_| "too long".to_string())?;
    let cred = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(name.as_mut_ptr()),
        UserName: PWSTR(user.as_mut_ptr()),
        CredentialBlobSize: size,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    // SAFETY: every pointer in `cred` points into a buffer that lives until the call returns.
    let written = unsafe { CredWriteW(&cred, 0) };
    // The copy of the secret isn't left in memory longer than needed.
    blob.fill(0);
    written.map_err(|e| e.message())
}

/// Removes what's kept under `target`; nothing there is fine.
pub fn delete(target: &str) -> Result<(), String> {
    let name = HSTRING::from(target);
    // SAFETY: a plain call with a NUL-terminated name.
    match unsafe { CredDeleteW(&name, CRED_TYPE_GENERIC, None) } {
        Ok(()) => Ok(()),
        // ERROR_NOT_FOUND: already gone.
        Err(e) if e.code().0 as u32 == 0x8007_0490 => Ok(()),
        Err(e) => Err(e.message()),
    }
}

/// The names of the entries that start with `prefix`.
pub fn list(prefix: &str) -> Vec<String> {
    let filter = HSTRING::from(format!("{prefix}*"));
    let mut count = 0u32;
    let mut creds: *mut *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: CredEnumerateW hands out an array of `count` credentials in one
    // block; it's read within that count and freed once.
    unsafe {
        if CredEnumerateW(&filter, None, &mut count, &mut creds).is_err() || creds.is_null() {
            return Vec::new();
        }
        let names = std::slice::from_raw_parts(creds, count as usize)
            .iter()
            .filter_map(|c| c.as_ref())
            .filter_map(|c| c.TargetName.to_string().ok())
            .filter(|n| n.starts_with(prefix))
            .collect();
        CredFree(creds.cast_const().cast());
        names
    }
}

#[cfg(test)]
mod tests {
    /// Kept by Trayward (UTF-8), or edited in Credential Manager (UTF-16).
    #[test]
    fn reads_a_secret_either_way() {
        assert_eq!(super::decode(b"glpat-fake").as_deref(), Some("glpat-fake"));
        let utf16: Vec<u8> = "glpat-fake".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(super::decode(&utf16).as_deref(), Some("glpat-fake"));
        // The end marks Credential Manager leaves behind go.
        let marked: Vec<u8> = "abc\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(super::decode(&marked).as_deref(), Some("abc"));
        // Neither UTF-16 (a lone surrogate) nor UTF-8: nothing.
        assert_eq!(super::decode(&[0x00, 0xD8, 0x41, 0x00]), None);
        assert_eq!(super::decode(&[0xFF, 0xFE, 0xFD]), None);
    }

    /// A made-up secret through this PC's Credential Manager: kept, read, listed, removed.
    #[test]
    #[ignore = "writes to this PC's Credential Manager"]
    fn keeps_and_forgets_a_secret() {
        let target = "trayward:test:example.com";
        super::write(target, "Trayward: test", "fake-secret-1234567890").unwrap();
        assert_eq!(super::read(target).as_deref(), Some("fake-secret-1234567890"));
        assert_eq!(super::list("trayward:test:"), [target]);
        super::delete(target).unwrap();
        assert_eq!(super::read(target), None);
        assert!(super::delete(target).is_ok(), "removing what's gone is fine");
    }
}
