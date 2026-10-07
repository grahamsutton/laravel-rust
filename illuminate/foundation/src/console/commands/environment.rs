//! `env:encrypt` and `env:decrypt` — commit an encrypted environment file
//! and decrypt it when you deploy.

use base64::Engine;
use illuminate_console::{Command, Console, async_trait};
use illuminate_encryption::Encrypter;
use illuminate_support::Result;

use crate::application::Application;

/// The environment file to work with: `.env`, or `.env.{env}`.
fn environment_file(cmd: &Console, app: &Application) -> String {
    match cmd.option("env").filter(|env| !env.is_empty()) {
        Some(env) => format!("{}/.env.{env}", app.environment_path().trim_end_matches('/')),
        None => app.environment_file_path(),
    }
}

/// Decode a key given as `base64:...` (or use it as-is).
fn parse_key(key: &str) -> Vec<u8> {
    match key.strip_prefix("base64:") {
        Some(encoded) => base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap_or_else(|_| key.as_bytes().to_vec()),
        None => key.as_bytes().to_vec(),
    }
}

/// The `NAME=value` lines of an environment file, skipping blanks and comments.
fn entries(contents: &str) -> impl Iterator<Item = (&str, &str)> {
    contents.lines().filter_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }
        line.split_once('=')
    })
}

/// `env:encrypt` — Encrypt an environment file.
pub struct EncryptCommand;

#[async_trait]
impl Command for EncryptCommand {
    fn signature(&self) -> &str {
        "env:encrypt
            {--key= : The encryption key}
            {--cipher= : The encryption cipher}
            {--env= : The environment to be encrypted}
            {--readable : Encrypt each variable individually with readable names, updating existing files and preserving unchanged values}
            {--prune : Delete the original environment file}
            {--force : Re-encrypt all values, overwriting the existing encrypted environment file}"
    }

    fn description(&self) -> &str {
        "Encrypt an environment file"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let cipher = cmd.option("cipher").filter(|c| !c.is_empty()).unwrap_or_else(|| "AES-256-CBC".into());
        let environment_file = environment_file(&cmd, &app);
        let encrypted_file = format!("{environment_file}.encrypted");

        let Ok(contents) = std::fs::read_to_string(&environment_file) else {
            return cmd.fail("Environment file not found.");
        };

        let encrypted_exists = std::path::Path::new(&encrypted_file).exists();
        let readable = cmd.option_bool("readable");
        let force = cmd.option_bool("force");
        let preserve = readable && encrypted_exists && !force;

        let given = cmd.option("key").filter(|key| !key.is_empty());
        if preserve && given.is_none() {
            return cmd.fail("The existing encryption key is required to update the encrypted environment file.");
        }
        if encrypted_exists && !force && !preserve {
            return cmd.fail("Encrypted environment file already exists.");
        }

        let key = match &given {
            Some(key) => parse_key(key),
            None => Encrypter::generate_key(&cipher),
        };
        let encrypter = match Encrypter::new(&key, &cipher) {
            Ok(encrypter) => encrypter,
            Err(error) => return cmd.fail(error.to_string()),
        };

        let encrypted = if readable {
            let previous = if preserve { std::fs::read_to_string(&encrypted_file).ok() } else { None };
            let mut existing: Vec<(String, String, String)> = Vec::new();
            if let Some(previous) = &previous {
                if Encrypter::appears_encrypted(previous.trim()) {
                    return cmd.fail("The existing encrypted environment file is not in readable format. Use --force to overwrite it.");
                }
                for (name, encrypted) in entries(previous) {
                    let Ok(value) = encrypter.decrypt_string(encrypted) else {
                        return cmd.fail("The given encryption key does not match the existing encrypted environment file.");
                    };
                    existing.push((name.to_string(), value, encrypted.to_string()));
                }
            }

            let mut result = String::new();
            for (name, value) in entries(&contents) {
                // Unchanged values keep their ciphertext, so diffs stay small.
                let reused = existing
                    .iter()
                    .position(|(existing_name, existing_value, _)| existing_name == name && existing_value == value)
                    .map(|index| existing.remove(index).2);
                let encrypted = match reused {
                    Some(encrypted) => encrypted,
                    None => encrypter.encrypt_string(value)?,
                };
                result.push_str(&format!("{name}={encrypted}\n"));
            }
            result
        } else {
            encrypter.encrypt(&contents)?
        };

        std::fs::write(&encrypted_file, encrypted)?;
        if cmd.option_bool("prune") {
            std::fs::remove_file(&environment_file)?;
        }

        let key = match given {
            Some(key) => key,
            None => format!("base64:{}", base64::engine::general_purpose::STANDARD.encode(&key)),
        };
        cmd.components().info("Environment successfully encrypted.");
        cmd.components().two_column_detail("Key", key);
        cmd.components().two_column_detail("Cipher", &cipher);
        cmd.components().two_column_detail("Encrypted file", &encrypted_file);
        cmd.new_line(1);
        Ok(())
    }
}

/// `env:decrypt` — Decrypt an environment file.
pub struct DecryptCommand;

#[async_trait]
impl Command for DecryptCommand {
    fn signature(&self) -> &str {
        "env:decrypt
            {--key= : The encryption key}
            {--cipher= : The encryption cipher}
            {--env= : The environment to be decrypted}
            {--force : Overwrite the existing environment file}
            {--path= : Path to write the decrypted file}
            {--filename= : Filename of the decrypted file}"
    }

    fn description(&self) -> &str {
        "Decrypt an environment file"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let Some(key) = cmd
            .option("key")
            .filter(|key| !key.is_empty())
            .or_else(|| std::env::var("LARAVEL_ENV_ENCRYPTION_KEY").ok().filter(|key| !key.is_empty()))
        else {
            return cmd.fail("A decryption key is required.");
        };
        let cipher = cmd
            .option("cipher")
            .filter(|c| !c.is_empty())
            .or_else(|| std::env::var("LARAVEL_ENV_ENCRYPTION_CIPHER").ok().filter(|c| !c.is_empty()))
            .unwrap_or_else(|| "AES-256-CBC".into());

        let environment_file = environment_file(&cmd, &app);
        let encrypted_file = format!("{environment_file}.encrypted");
        let output_file = match (cmd.option("path").filter(|p| !p.is_empty()), cmd.option("filename").filter(|f| !f.is_empty())) {
            (path, Some(filename)) => format!(
                "{}/{filename}",
                path.unwrap_or_else(|| app.environment_path()).trim_end_matches('/')
            ),
            (Some(path), None) => format!(
                "{}/{}",
                path.trim_end_matches('/'),
                std::path::Path::new(&environment_file).file_name().unwrap_or_default().to_string_lossy()
            ),
            (None, None) => environment_file.clone(),
        };

        let Ok(contents) = std::fs::read_to_string(&encrypted_file) else {
            return cmd.fail("Encrypted environment file not found.");
        };
        if std::path::Path::new(&output_file).exists() && !cmd.option_bool("force") {
            return cmd.fail("Environment file already exists.");
        }

        let encrypter = match Encrypter::new(parse_key(&key), &cipher) {
            Ok(encrypter) => encrypter,
            Err(error) => return cmd.fail(error.to_string()),
        };

        let decrypted = if Encrypter::appears_encrypted(contents.trim()) {
            match encrypter.decrypt::<String>(contents.trim()) {
                Ok(decrypted) => decrypted,
                Err(error) => return cmd.fail(error.to_string()),
            }
        } else {
            let mut result = String::new();
            for (name, encrypted) in entries(&contents) {
                match encrypter.decrypt_string(encrypted) {
                    Ok(value) => result.push_str(&format!("{name}={value}\n")),
                    Err(error) => return cmd.fail(error.to_string()),
                }
            }
            result
        };

        std::fs::write(&output_file, decrypted)?;
        cmd.components().info("Environment successfully decrypted.");
        cmd.components().two_column_detail("Decrypted file", &output_file);
        cmd.new_line(1);
        Ok(())
    }
}
