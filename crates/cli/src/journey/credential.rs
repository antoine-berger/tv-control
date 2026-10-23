//! Explicit local credential choice. Never infer persistence from a live process.
use super::{guidance, Journey};
use crate::terminal::Terminal;
use std::path::Path;
use tv_application::Context;

pub(super) fn available(
    state: &Path,
    installation: &str,
    tunnel: &str,
) -> Result<Option<String>, &'static str> {
    // Validate stored state even when the caller supplies an override. An
    // explicit environment key can recover an expired key without deleting state.
    let saved = tv_adapters::runtime_credential::load(state, installation, tunnel)?;
    match std::env::var("TV_CONTROL_TUNNEL_KEY") {
        Ok(key) if !key.is_empty() => {
            validate(&key)?;
            Ok(Some(key))
        }
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(saved),
        Err(_) => Err("runtime_key_invalid"),
    }
}
fn validate(key: &str) -> Result<(), &'static str> {
    if key.is_empty() || key.len() > 4096 || key.chars().any(char::is_control) {
        Err("runtime_key_invalid")
    } else {
        Ok(())
    }
}
pub(super) fn acquire(
    state: &Path,
    installation: &str,
    tunnel: &str,
    terminal: &Terminal,
    ctx: &Context<'_>,
) -> Result<String, &'static str> {
    if let Some(key) = available(state, installation, tunnel)? {
        let saved = tv_adapters::runtime_credential::load(state, installation, tunnel)?;
        if saved.as_deref() != Some(key.as_str()) {
            save_choice(state, installation, tunnel, &key, terminal, ctx)?;
        }
        return Ok(key);
    }
    eprintln!("No saved runtime key was found for this connection. Older releases kept it only in the running process; they did not save it in your TV configuration. A stopped process cannot supply it again. Create a replacement runtime key for this same tunnel if the original is unavailable; do not paste an administrator key.");
    guidance::runtime_key();
    let key = terminal
        .line(
            "Runtime key (hidden; q cancels without stopping your controller): ",
            true,
            ctx,
        )?
        .ok_or("cancelled")?;
    validate(&key)?;
    save_choice(state, installation, tunnel, &key, terminal, ctx)?;
    Ok(key)
}
fn save_choice(
    state: &Path,
    installation: &str,
    tunnel: &str,
    key: &str,
    terminal: &Terminal,
    ctx: &Context<'_>,
) -> Result<(), &'static str> {
    let choice = terminal.line("Save this runtime key in a private local file for future updates? File permissions restrict it to your user; it is not encrypted. [y/N] ", false, ctx)?.ok_or("cancelled")?;
    if choice == "y" {
        tv_adapters::runtime_credential::save(state, installation, tunnel, key, true)?;
        eprintln!("Runtime key saved privately for this connection. It is not part of the TV configuration or logs.");
    } else {
        eprintln!("This runtime key was not saved. A later restart needs a valid saved key or TV_CONTROL_TUNNEL_KEY.");
    }
    Ok(())
}

impl Journey<'_> {
    pub(super) fn acquire_key(
        &self,
        tunnel: &str,
        ctx: &Context<'_>,
    ) -> Result<String, &'static str> {
        acquire(&self.state, &self.installation, tunnel, self.terminal, ctx)
    }
}
