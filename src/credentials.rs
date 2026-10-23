use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use argon2::Argon2;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

pub const KEY_ENV: &str = "DEEPSEEK_API_KEY";
pub const OPENCODE_KEY_ENV: &str = "OPENCODE_API_KEY";

const FILE_VERSION: u8 = 1;
const KDF_SALT: &[u8] = b"seekfetch credential v1";
const NONCE_LEN: usize = 24;

/// One provider's encrypted credential file and its environment override.
#[derive(Debug, Clone, Copy)]
pub struct KeyStore {
    label: &'static str,
    env_var: &'static str,
    file_name: &'static str,
    set_key_command: &'static str,
    import_command: &'static str,
}

impl KeyStore {
    pub const DEEPSEEK: Self = Self {
        label: "DeepSeek",
        env_var: KEY_ENV,
        file_name: "credentials",
        set_key_command: "seekfetch config set-key",
        import_command: "seekfetch config import",
    };

    pub const OPENCODE: Self = Self {
        label: "OpenCode Go",
        env_var: OPENCODE_KEY_ENV,
        file_name: "credentials-opencode",
        set_key_command: "seekfetch opencode set-key",
        import_command: "seekfetch opencode import",
    };

    pub fn env_var(&self) -> &'static str {
        self.env_var
    }

    pub fn path(&self) -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .context("cannot locate the platform config directory")?
            .join("seekfetch");
        Ok(dir.join(self.file_name))
    }

    /// Key from the environment override, ignoring blank values.
    pub fn env_key(&self) -> Option<String> {
        std::env::var(self.env_var)
            .ok()
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
    }

    pub fn load(&self) -> Result<String> {
        if let Some(key) = self.env_key() {
            return Ok(key);
        }

        let path = self.path()?;
        if !path.exists() {
            bail!("{}", self.missing_key_message());
        }

        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        open(&machine_key()?, text.trim()).with_context(|| {
            format!(
                "failed to decrypt the {} credential file; the machine key may have changed, run `{}` again",
                self.label, self.set_key_command
            )
        })
    }

    pub fn set_key(&self, stdin: bool, from_env: Option<&str>) -> Result<()> {
        let key = if let Some(name) = from_env {
            std::env::var(name)
                .with_context(|| format!("environment variable {name} is not set"))?
        } else if stdin || !std::io::stdin().is_terminal() {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("failed to read the API key from stdin")?;
            buf
        } else {
            rpassword::prompt_password(format!("{} API key: ", self.label))
                .context("failed to read the API key")?
        };

        let key = normalize_key(&key)?;
        self.store(&key)
    }

    pub fn import_dotenv(&self, dotenv: &Path, name: &str) -> Result<()> {
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
        self.store(&key)
    }

    pub fn clear(&self) -> Result<()> {
        let path = self.path()?;
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
            println!("removed {}", path.display());
        } else {
            println!("no credential file at {}", path.display());
        }
        Ok(())
    }

    fn store(&self, key: &str) -> Result<()> {
        let path = self.path()?;
        let dir = path
            .parent()
            .context("credential path has no parent directory")?;
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;

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

        println!(
            "stored encrypted {} credential at {}",
            self.label,
            path.display()
        );
        Ok(())
    }

    fn missing_key_message(&self) -> String {
        format!(
            "no {} API key configured; set one first:\n  \
             {}                   (interactive prompt, or pipe via stdin)\n  \
             {} --dotenv <FILE>    (import from a dotenv file)\n  \
             or export {}=YOUR_API_KEY    (environment override)",
            self.label, self.set_key_command, self.import_command, self.env_var
        )
    }
}

pub fn path() -> Result<PathBuf> {
    KeyStore::DEEPSEEK.path()
}

pub fn set_key(stdin: bool, from_env: Option<&str>) -> Result<()> {
    KeyStore::DEEPSEEK.set_key(stdin, from_env)
}

pub fn import_dotenv(dotenv: &Path, name: &str) -> Result<()> {
    KeyStore::DEEPSEEK.import_dotenv(dotenv, name)
}

pub fn clear() -> Result<()> {
    KeyStore::DEEPSEEK.clear()
}

pub fn load_key() -> Result<String> {
    KeyStore::DEEPSEEK.load()
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

pub fn mask_key(key: &str) -> String {
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

    #[test]
    fn providers_use_separate_files_and_env_vars() {
        assert_eq!(KeyStore::DEEPSEEK.file_name, "credentials");
        assert_eq!(KeyStore::OPENCODE.file_name, "credentials-opencode");
        assert_eq!(KeyStore::DEEPSEEK.env_var, "DEEPSEEK_API_KEY");
        assert_eq!(KeyStore::OPENCODE.env_var, "OPENCODE_API_KEY");
        assert_ne!(KeyStore::DEEPSEEK.label, KeyStore::OPENCODE.label);
    }

    #[test]
    fn missing_key_message_names_the_provider_commands() {
        let message = KeyStore::OPENCODE.missing_key_message();
        assert!(message.contains("OpenCode Go"));
        assert!(message.contains("seekfetch opencode set-key"));
        assert!(message.contains("OPENCODE_API_KEY"));
    }
}
