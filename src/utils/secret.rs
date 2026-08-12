use keyring::{Entry, Result};

use super::load::AiConfig;

const SERVICE: &str = "seam";

/// Secure credential access backed by the operating system keyring.
pub struct SecretManager {
  entry: Entry,
}

impl SecretManager {
  pub fn new(provider: &str) -> Result<Self> {
    Ok(Self {
      entry: Entry::new(SERVICE, credential_target(provider))?,
    })
  }

  pub fn for_ai(config: &AiConfig) -> Result<Self> {
    Self::new(&config.provider)
  }

  /// Store or replace the provider credential (API key or OAuth token).
  pub fn save_key(&self, secret: &str) -> Result<()> {
    self.entry.set_password(secret)
  }

  /// Retrieve the provider credential without logging or displaying it.
  pub fn load_key(&self) -> Result<String> {
    self.entry.get_password()
  }

  pub fn delete_key(&self) -> Result<()> {
    self.entry.delete_credential()
  }
}

/// Load the credential belonging to the provider selected in config.toml.
pub fn load_ai_credential(config: &AiConfig) -> Result<String> {
  SecretManager::for_ai(config)?.load_key()
}

fn credential_target(provider: &str) -> &str {
  match provider {
    "chatgpt" => "chatgpt-oauth",
    provider => provider,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn chatgpt_oauth_is_separate_from_openai_api_key() {
    assert_eq!(credential_target("chatgpt"), "chatgpt-oauth");
    assert_eq!(credential_target("openai"), "openai");
    assert_eq!(credential_target("gemini"), "gemini");
  }
}
