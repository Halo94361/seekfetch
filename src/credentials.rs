use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use argon2::Argon2;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

pub const KEY_ENV: &str = "DEEPSEEK_API_KEY";

const FILE_VERSION: u8 = 1;
const KDF_SALT: &[u8] = b"seekfetch credential v1";
const NONCE_LEN: usize = 24;

pub fn path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("cannot locate the platform config directory")?
        .join("seekfetch");
    Ok(dir.join("credentials"))
}

pub fn set_key(stdin: bool, from_env: Option<&str>) -> Result<()> {
    let key = if let Some(name) = from_env {
        std::env::var(name).with_context(|| format!("environment variable {name} is not set"))?
    } else if stdin || !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read the API key from stdin")?;
        buf
    } else {
        rpassword::prompt_password("DeepSeek API key: ").context("failed to read the API key")?
    };

    let key = normalize_key(&key)?;
    store(&key)
}

pub fn import_dotenv(dotenv: &Path, name: &str) -> Result<()> {
    let text = std::fs::read_to_string(dotenv)
        .with_context(|| format!("failed to read {}", dotenv.display()))?;

    let value = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            (key.trim() == name).then(|| value.to_string())
        })
        .next()
        .with_context(|| format!("{name} not found in {}", dotenv.display()))?;

    let key = normalize_key(&value)?;
    store(&key)
}

pub fn show() -> Result<()> {
    let path = path()?;
    println!("credentials: {}", path.display());
    match load_key() {
        Ok(key) => println!("api key: {}", mask_key(&key)),
        Err(err) => println!("api key: {err}"),
    }
    Ok(())
}

pub fn clear() -> Result<()> {
    let path = path()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
        println!("removed {}", path.display());
    } else {
        println!("no credential file at {}", path.display());
    }
    Ok(())
}

pub fn load_key() -> Result<String> {
    if let Ok(key) = std::env::var(KEY_ENV) {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }

    let path = path()?;
    if !path.exists() {
        bail!("no API key configured; run `seekfetch config set-key`");
    }

    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    open(&machine_key()?, text.trim())
        .context("failed to decrypt the credential file; the machine key may have changed, run `seekfetch config set-key` again")
}

fn store(key: &str) -> Result<()> {
    let path = path()?;
    let dir = path
        .parent()
        .context("credential path has no parent directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let sealed = seal(&machine_key()?, key)?;
    std::fs::write(&path, format!("{sealed}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to set permissions on {}", dir.display()))?;
    }

    println!("stored encrypted credential at {}", path.display());
    Ok(())
}

fn machine_key() -> Result<[u8; 32]> {
    let material = format!("{}\n{}", machine_id(), user_name());
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(material.as_bytes(), KDF_SALT, &mut key)
        .map_err(|err| anyhow::anyhow!("failed to derive the machine key: {err}"))?;
    Ok(key)
}

#[cfg(unix)]
fn machine_id() -> String {
    for candidate in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(id) = std::fs::read_to_string(candidate) {
            let id = id.trim();
            if !id.is_empty() {
                return id.to_string();
            }
        }
    }
    "unknown-machine".to_string()
}

#[cfg(windows)]
fn machine_id() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown-machine".to_string())
}

fn user_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown-user".to_string())
}

fn seal(key: &[u8; 32], plaintext: &str) -> Result<String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce: [u8; NONCE_LEN] = rand::random();
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext.as_bytes())
        .map_err(|err| anyhow::anyhow!("failed to encrypt the credential: {err}"))?;

    let mut blob = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
    blob.push(FILE_VERSION);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    Ok(B64.encode(blob))
}

fn open(key: &[u8; 32], encoded: &str) -> Result<String> {
    let blob = B64.decode(encoded).context("credential file is corrupt")?;
    if blob.len() <= 1 + NONCE_LEN || blob[0] != FILE_VERSION {
        bail!("credential file has an unsupported format");
    }

    let cipher = XChaCha20Poly1305::new(key.into());
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(&blob[1..1 + NONCE_LEN]),
            &blob[1 + NONCE_LEN..],
        )
        .map_err(|_| anyhow::anyhow!("decryption failed"))?;
    String::from_utf8(plaintext).context("credential file is corrupt")
}

fn normalize_key(raw: &str) -> Result<String> {
    let key = raw.trim();
    if key.is_empty() {
        bail!("API key is empty");
    }
    if key.contains(['\r', '\n']) {
        bail!("API key must be a single line");
    }
    Ok(key.to_string())
}

fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("****{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        [7u8; 32]
    }

    #[test]
    fn seal_open_roundtrip() {
        let key = test_key();
        let sealed = seal(&key, "sk-test-secret").unwrap();
        assert_eq!(open(&key, &sealed).unwrap(), "sk-test-secret");
    }

    #[test]
    fn open_rejects_wrong_key() {
        let sealed = seal(&test_key(), "sk-test-secret").unwrap();
        assert!(open(&[9u8; 32], &sealed).is_err());
    }

    #[test]
    fn open_rejects_corrupt_data() {
        assert!(open(&test_key(), "not-base64!!!").is_err());
        assert!(open(&test_key(), &B64.encode([FILE_VERSION, 1, 2])).is_err());
    }

    #[test]
    fn open_rejects_unknown_version() {
        let key = test_key();
        let sealed = seal(&key, "sk-test-secret").unwrap();
        let mut blob = B64.decode(&sealed).unwrap();
        blob[0] = FILE_VERSION + 1;
        assert!(open(&key, &B64.encode(blob)).is_err());
    }

    #[test]
    fn normalize_trims_and_rejects_multiline() {
        assert_eq!(normalize_key("  sk-abc \n").unwrap(), "sk-abc");
        assert!(normalize_key("   ").is_err());
        assert!(normalize_key("sk-a\nsk-b").is_err());
    }

    #[test]
    fn mask_shows_only_tail() {
        assert_eq!(mask_key("sk-1234567890abcd"), "****abcd");
        assert_eq!(mask_key("short"), "****");
    }
}
