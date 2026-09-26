//! Password-based encryption of the sync file, and keeping the sync password on this device.
//!
//! The payload is sealed with AES-256-GCM under a key derived from the password with
//! Argon2id, with a fresh random salt and nonce on every export. The plaintext header (format,
//! export time, machine, KDF parameters) is authenticated as associated data: it can be shown
//! before unlocking, but changing it makes decryption fail.
//!
//! The password is kept in `store.json` → `sync.password`, protected with DPAPI on Windows
//! (only this Windows account can read it back); elsewhere it relies on the store's 0600 mode.

use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use serde_json::{json, Value};
use std::sync::Mutex;

pub const CIPHER: &str = "aes-256-gcm";
pub const KDF: &str = "argon2id";
/// Argon2id cost for new files: 64 MiB, 3 passes (well above OWASP's minimum).
const COST: Cost = if cfg!(test) { Cost { m: 8 * 1024, t: 1, p: 1 } } else { Cost { m: 64 * 1024, t: 3, p: 1 } };
/// Shortest password accepted. A generated key is much longer.
pub const MIN_LEN: usize = 8;
const SALT_LEN: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cost {
    /// Memory in KiB.
    pub m: u32,
    pub t: u32,
    pub p: u32,
}

impl Cost {
    /// A file sets its own cost: refuse values that would take minutes or gigabytes to try
    /// (a tampered file must not be able to hang or exhaust this machine), or too weak to trust.
    fn check(self) -> Result<Self> {
        if (8 * 1024..=256 * 1024).contains(&self.m) && (1..=10).contains(&self.t) && (1..=8).contains(&self.p) {
            Ok(self)
        } else {
            bail!("{}", crate::i18n::l("The sync file uses unsupported encryption settings", "同步文件的加密参数不受支持"))
        }
    }
}

/// The header fields of a sealed file (all but the ciphertext).
#[derive(Clone, PartialEq, Debug)]
pub struct Header {
    pub cost: Cost,
    pub salt: Vec<u8>,
    pub nonce: [u8; NONCE_LEN],
}

impl Header {
    /// The `encryption` object written into the file, in a fixed field order (it is part of the AAD).
    pub fn to_json(&self) -> Value {
        json!({ "cipher": CIPHER, "kdf": KDF, "m": self.cost.m, "t": self.cost.t, "p": self.cost.p, "salt": B64.encode(&self.salt), "nonce": B64.encode(self.nonce) })
    }

    pub fn from_json(v: &Value) -> Result<Header> {
        let bad = || anyhow!(crate::i18n::l("The sync file's encryption header is damaged", "同步文件的加密信息已损坏"));
        if v["cipher"] != CIPHER || v["kdf"] != KDF {
            bail!("{}", crate::i18n::l("The sync file uses unsupported encryption settings", "同步文件的加密参数不受支持"));
        }
        let num = |k: &str| v[k].as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(bad);
        let cost = Cost { m: num("m")?, t: num("t")?, p: num("p")? }.check()?;
        let salt = v["salt"].as_str().and_then(|s| B64.decode(s).ok()).filter(|s| (SALT_LEN..=64).contains(&s.len())).ok_or_else(bad)?;
        let nonce = v["nonce"].as_str().and_then(|s| B64.decode(s).ok()).and_then(|n| <[u8; NONCE_LEN]>::try_from(n).ok()).ok_or_else(bad)?;
        Ok(Header { cost, salt, nonce })
    }
}

pub fn random<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).map_err(|e| anyhow!(tr!("Couldn't get random bytes: {e}", "获取随机数失败：{e}")))?;
    Ok(b)
}

/// The last derived key: an export followed by imports and applies would otherwise
/// run Argon2 (a noticeable fraction of a second) again for the same file.
static LAST: Mutex<Option<Derived>> = Mutex::new(None);

/// (password, cost, salt) → key.
type Derived = (String, Cost, Vec<u8>, [u8; 32]);

fn derive(password: &str, cost: Cost, salt: &[u8]) -> Result<[u8; 32]> {
    if let Some((p, c, s, k)) = crate::util::lock(&LAST).as_ref() {
        if p == password && *c == cost && s == salt {
            return Ok(*k);
        }
    }
    let params = argon2::Params::new(cost.m, cost.t, cost.p, Some(32)).map_err(|e| anyhow!("argon2: {e}"))?;
    let mut key = [0u8; 32];
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("argon2: {e}"))?;
    *crate::util::lock(&LAST) = Some((password.to_string(), cost, salt.to_vec(), key));
    Ok(key)
}

fn aead(key: &[u8; 32]) -> Result<LessSafeKey> {
    Ok(LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| anyhow!("aes-256-gcm"))?))
}

/// Encrypts `plain` under `password`. `aad` gets the header returned here: callers build the
/// authenticated bytes from it (see `sync::aad`).
pub fn seal(password: &str, plain: &[u8], aad: impl FnOnce(&Header) -> Vec<u8>) -> Result<(Header, Vec<u8>)> {
    let h = Header { cost: COST, salt: random::<SALT_LEN>()?.to_vec(), nonce: random::<NONCE_LEN>()? };
    let key = derive(password, h.cost, &h.salt)?;
    let mut buf = plain.to_vec();
    aead(&key)?
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(h.nonce), Aad::from(aad(&h)), &mut buf)
        .map_err(|_| anyhow!("aes-256-gcm"))?;
    Ok((h, buf))
}

/// Decrypts; a wrong password and a modified file fail the same way.
pub fn open(password: &str, h: &Header, aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>> {
    let key = derive(password, h.cost, &h.salt)?;
    let mut buf = sealed.to_vec();
    let n = aead(&key)?
        .open_in_place(Nonce::assume_unique_for_key(h.nonce), Aad::from(aad), &mut buf)
        .map_err(|_| anyhow!(crate::i18n::l("Wrong sync password, or the sync file has been modified", "同步密码不对，或者同步文件被改动过")))?
        .len();
    buf.truncate(n);
    Ok(buf)
}

/// A random key to use as the sync password: 160 bits in Crockford base32, in groups of four.
pub fn generate_key() -> Result<String> {
    const ABC: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let bytes = random::<20>()?;
    let mut bits: u32 = 0;
    let mut n = 0;
    let mut out = String::new();
    for b in bytes {
        bits = (bits << 8) | b as u32;
        n += 8;
        while n >= 5 {
            n -= 5;
            if !out.is_empty() && out.len() % 5 == 4 {
                out.push('-');
            }
            out.push(ABC[((bits >> n) & 31) as usize] as char);
        }
    }
    Ok(out)
}

/// Checks a password the user typed; returns it trimmed (a copied key often brings spaces).
pub fn check_password(p: &str) -> Result<String> {
    let p = p.trim();
    if p.chars().count() < MIN_LEN {
        bail!("{}", tr!("The sync password needs at least {MIN_LEN} characters", "同步密码至少要 {MIN_LEN} 个字符"));
    }
    Ok(p.to_string())
}

// ------------------------------------------------------------ keeping the password

/// Whether a saved password is protected by the OS (DPAPI) rather than only by file permissions.
pub const SYSTEM_PROTECTED: bool = cfg!(windows);

/// The form stored in `store.json`.
pub fn protect(password: &str) -> Result<Value> {
    #[cfg(windows)]
    {
        Ok(json!({ "dpapi": B64.encode(dpapi::protect(password.as_bytes())?) }))
    }
    #[cfg(not(windows))]
    {
        Ok(json!({ "plain": password }))
    }
}

pub fn unprotect(v: &Value) -> Result<String> {
    if let Some(s) = v["plain"].as_str() {
        return Ok(s.to_string());
    }
    #[cfg(windows)]
    if let Some(b) = v["dpapi"].as_str() {
        let bytes = B64.decode(b).map_err(|e| anyhow!("{e}"))?;
        return String::from_utf8(dpapi::unprotect(&bytes)?).map_err(|e| anyhow!("{e}"));
    }
    bail!(
        "{}",
        crate::i18n::l("The saved sync password can't be read on this device (it was saved by another Windows account?). Enter it again", "本机保存的同步密码读不出来（可能是其他 Windows 账户保存的），请重新输入")
    )
}

#[cfg(windows)]
mod dpapi {
    use anyhow::{anyhow, Result};
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

    /// Ties the blob to AgentPlus, so another program's DPAPI blob isn't accepted as ours.
    const ENTROPY: &[u8] = b"AgentPlus sync password v1";

    fn run(data: &[u8], encrypt: bool) -> Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: the input blobs point at live slices that DPAPI only reads; the output is
        // allocated by DPAPI, copied out, then released with LocalFree as documented.
        unsafe {
            let r = if encrypt {
                CryptProtectData(&input, windows::core::PCWSTR::null(), Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            } else {
                CryptUnprotectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            };
            r.map_err(|e| anyhow!("DPAPI: {e}"))?;
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(HLOCAL(out.pbData as *mut core::ffi::c_void));
            Ok(bytes)
        }
    }

    pub fn protect(data: &[u8]) -> Result<Vec<u8>> {
        run(data, true)
    }

    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
        run(data, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aad(h: &Header) -> Vec<u8> {
        serde_json::to_vec(&h.to_json()).unwrap()
    }

    #[test]
    fn seal_then_open_round_trips() {
        let (h, sealed) = seal("correct horse", b"hello", aad).unwrap();
        assert_ne!(&sealed[..5], b"hello");
        assert_eq!(open("correct horse", &h, &aad(&h), &sealed).unwrap(), b"hello");
    }

    #[test]
    fn wrong_password_or_changed_data_fails() {
        let (h, sealed) = seal("correct horse", b"hello", aad).unwrap();
        assert!(open("wrong horse", &h, &aad(&h), &sealed).is_err());
        let mut bad = sealed.clone();
        bad[0] ^= 1;
        assert!(open("correct horse", &h, &aad(&h), &bad).is_err());
        // The header is authenticated too.
        assert!(open("correct horse", &h, b"another header", &sealed).is_err());
    }

    #[test]
    fn every_seal_uses_fresh_salt_and_nonce() {
        let (a, x) = seal("pw-12345678", b"same", aad).unwrap();
        let (b, y) = seal("pw-12345678", b"same", aad).unwrap();
        assert_ne!(a.salt, b.salt);
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(x, y);
    }

    #[test]
    fn header_round_trips_and_rejects_bad_values() {
        let h = Header { cost: COST, salt: vec![7; 16], nonce: [1; NONCE_LEN] };
        assert_eq!(Header::from_json(&h.to_json()).unwrap(), h);
        for (k, v) in [("m", json!(4 * 1024 * 1024)), ("t", json!(0)), ("p", json!(64)), ("cipher", json!("rot13")), ("salt", json!("AAAA")), ("nonce", json!("AA=="))] {
            let mut j = h.to_json();
            j[k] = v;
            assert!(Header::from_json(&j).is_err(), "{k}");
        }
    }

    #[test]
    fn generated_keys_are_long_and_readable() {
        let k = generate_key().unwrap();
        assert_eq!(k.len(), 32 + 7, "{k}");
        assert!(k.split('-').all(|g| g.len() == 4 && g.chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())), "{k}");
        assert_ne!(k, generate_key().unwrap());
        assert!(check_password(&k).is_ok());
    }

    #[test]
    fn passwords_are_trimmed_and_checked() {
        assert_eq!(check_password("  abcdefgh \n").unwrap(), "abcdefgh");
        assert!(check_password("short").is_err());
        assert!(check_password("   ").is_err());
        // Characters, not bytes.
        assert!(check_password("密码密码密码密码").is_ok());
    }

    #[test]
    fn protected_password_round_trips() {
        let v = protect("s3cret-pass").unwrap();
        assert!(!v.to_string().contains("s3cret-pass") || !SYSTEM_PROTECTED);
        assert_eq!(unprotect(&v).unwrap(), "s3cret-pass");
        assert!(unprotect(&json!({})).is_err());
    }
}
