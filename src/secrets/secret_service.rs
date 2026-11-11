//! The Secret Service (GNOME Keyring or KWallet), through oo7: on the host
//! and, through the secret portal, inside Flatpak.

use zeroize::Zeroizing;

use super::{KEYRING_APP, Secret};

/// The Secret Service's default collection. `oo7::Keyring::new` would pick
/// a file in the sandbox inside Flatpak.
async fn keyring() -> Result<oo7::Keyring, String> {
    let service = oo7::dbus::Service::new().await.map_err(|e| e.to_string())?;
    let collection = service
        .default_collection()
        .await
        .map_err(|e| e.to_string())?;
    Ok(oo7::Keyring::DBus(collection))
}

/// Inside Flatpak, move the passwords an earlier version kept in the
/// sandbox's keyring file to the Secret Service, leaving any already
/// there. The file is renamed once its passwords are across.
pub(crate) async fn move_from_sandbox() -> Result<(), String> {
    if std::env::var_os("FLATPAK_ID").is_none() {
        return Ok(());
    }
    // Where oo7 0.6 keeps it: `oo7::Keyring::new` below opens this file.
    let file = gtk::glib::user_data_dir().join("keyrings/default.keyring");
    if !file.is_file() {
        return Ok(());
    }
    // In the sandbox, this is that file, opened with the portal's key.
    let sandbox = oo7::Keyring::new().await.map_err(|e| e.to_string())?;
    if matches!(sandbox, oo7::Keyring::DBus(_)) {
        return Ok(());
    }
    let host = keyring().await?;
    let items = sandbox
        .search_items(&[("application", KEYRING_APP)])
        .await
        .map_err(|e| e.to_string())?;
    let mut moved = 0;
    for item in items {
        let attributes = item.attributes().await.map_err(|e| e.to_string())?;
        let there = host
            .search_items(&attributes)
            .await
            .map_err(|e| e.to_string())?;
        if there.is_empty() {
            let label = item.label().await.map_err(|e| e.to_string())?;
            let secret = item.secret().await.map_err(|e| e.to_string())?;
            host.create_item(&label, &attributes, secret, false)
                .await
                .map_err(|e| e.to_string())?;
            moved += 1;
        }
    }
    std::fs::rename(&file, file.with_extension("keyring.moved")).map_err(|e| e.to_string())?;
    tracing::info!("moved {moved} passwords from the sandbox's keyring");
    Ok(())
}

fn attributes(id: &str, kind: Secret) -> [(&'static str, &str); 2] {
    [("application", KEYRING_APP), (kind.attribute(), id)]
}

/// The saved password's bytes, or `None` when there is none.
pub(super) async fn get(id: &str, kind: Secret) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    let keyring = keyring().await?;
    let items = keyring
        .search_items(&attributes(id, kind))
        .await
        .map_err(|e| e.to_string())?;
    let Some(item) = items.first() else {
        return Ok(None);
    };
    let secret = item.secret().await.map_err(|e| e.to_string())?;
    Ok(Some(Zeroizing::new(secret.to_vec())))
}

pub(super) async fn set(id: &str, kind: Secret, label: &str, password: &str) -> Result<(), String> {
    keyring()
        .await?
        .create_item(label, &attributes(id, kind), password, true)
        .await
        .map_err(|e| e.to_string())
}

/// Forget the password, if there is one.
pub(super) async fn delete(id: &str, kind: Secret) -> Result<(), String> {
    keyring()
        .await?
        .delete(&attributes(id, kind))
        .await
        .map_err(|e| e.to_string())
}
