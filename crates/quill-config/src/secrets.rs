use crate::models::Provider;

const SERVICE: &str = "quill-code";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeySource {
    Stored,
    Environment(&'static str),
    Missing,
}

fn entry(id: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, id).ok()
}

fn stored(id: &str) -> Option<String> {
    entry(id)?.get_password().ok().filter(|key| !key.trim().is_empty())
}

fn environment(provider: &Provider) -> Option<(&'static str, String)> {
    let name = provider.kind.env_key().filter(|_| provider.official())?;
    std::env::var(name).ok().filter(|key| !key.trim().is_empty()).map(|key| (name, key))
}

pub fn api_key(provider: &Provider) -> Option<String> {
    stored(&provider.id).or_else(|| environment(provider).map(|(_, key)| key))
}

pub fn key_source(provider: &Provider) -> KeySource {
    if stored(&provider.id).is_some() {
        KeySource::Stored
    } else if let Some((name, _)) = environment(provider) {
        KeySource::Environment(name)
    } else {
        KeySource::Missing
    }
}

pub fn store_key(id: &str, key: &str) -> Result<(), String> {
    let entry = entry(id).ok_or("Windows Credential Manager is not available.")?;
    entry.set_password(key.trim()).map_err(|e| format!("The key could not be saved: {e}"))
}

pub fn forget_key(id: &str) -> Result<(), String> {
    match entry(id).map(|entry| entry.delete_credential()) {
        Some(Err(keyring::Error::NoEntry)) | Some(Ok(())) | None => Ok(()),
        Some(Err(error)) => Err(format!("The key could not be removed: {error}")),
    }
}