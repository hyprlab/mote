//! Connection passwords, in the desktop's Secret Service (GNOME Keyring or
//! KWallet), for every kind of install: the Flatpak talks to it too, rather
//! than keeping a keyring of its own in its sandbox, so a package and the
//! Flatpak find the same passwords. On macOS they are in the Keychain. Each
//! item is found by the connection's id, so renaming a connection keeps its
//! password.
//!
//! Everything here is async and runs on relm4's command runtime, never on
//! the GTK main thread: an unlock prompt can take as long as the user does.

use zeroize::Zeroizing;

#[cfg(target_os = "macos")]
mod keychain;
#[cfg(not(target_os = "macos"))]
mod secret_service;

#[cfg(target_os = "macos")]
use keychain as store;
#[cfg(not(target_os = "macos"))]
use secret_service as store;
#[cfg(not(target_os = "macos"))]
pub(crate) use secret_service::move_from_sandbox;

/// Who the keyring's items belong to. The stable app's ID in the beta app
/// too, which shares its connections and so needs their passwords.
const KEYRING_APP: &str = "co.hyprlab.Mote";

/// How the keyring's items name the app, for the same reason.
const KEYRING_NAME: &str = "Mote";

/// A password, wiped from memory when dropped.
pub(crate) type Password = Zeroizing<String>;

/// Which of a connection's passwords.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Secret {
    /// The account on the remote desktop.
    Account,
    /// The account on the SSH server the connection goes through.
    Ssh,
    /// The account on the RD Gateway.
    Gateway,
}

impl Secret {
    pub(crate) const ALL: [Self; 3] = [Self::Account, Self::Ssh, Self::Gateway];

    /// The attribute naming the connection. Each kind has its own, so a
    /// search for one never finds another: the account's is the one
    /// passwords were saved under before the others existed.
    fn attribute(self) -> &'static str {
        match self {
            Self::Account => "connection",
            Self::Ssh => "ssh-tunnel",
            Self::Gateway => "rd-gateway",
        }
    }

    /// The name the keyring shows for it.
    pub(crate) fn label(self, title: &str) -> String {
        match self {
            Self::Account => format!("{title} ({KEYRING_NAME})"),
            Self::Ssh => format!("{title}, SSH tunnel ({KEYRING_NAME})"),
            Self::Gateway => format!("{title}, RD Gateway ({KEYRING_NAME})"),
        }
    }
}

/// The saved password, or `None` when there is none.
pub(crate) async fn password(id: String, kind: Secret) -> Result<Option<Password>, String> {
    let Some(mut secret) = store::get(&id, kind).await? else {
        return Ok(None);
    };
    // The bytes move into the string, or are wiped when they aren't text.
    match String::from_utf8(std::mem::take(&mut *secret)) {
        Ok(text) => Ok(Some(Zeroizing::new(text))),
        Err(e) => {
            drop(Zeroizing::new(e.into_bytes()));
            Err("the saved password is not text".to_owned())
        }
    }
}

/// Save `password` for the connection, or forget it when it is empty.
pub(crate) async fn set_password(
    id: String,
    kind: Secret,
    title: String,
    password: Password,
) -> Result<(), String> {
    if password.is_empty() {
        return store::delete(&id, kind).await;
    }
    store::set(&id, kind, &kind.label(&title), &password).await
}

/// Forget every password the connection has.
pub(crate) async fn delete_passwords(id: String) -> Result<(), String> {
    for kind in Secret::ALL {
        store::delete(&id, kind).await?;
    }
    Ok(())
}

/// Give the connection `to` the passwords `from` has.
pub(crate) async fn copy_passwords(from: String, to: String, title: String) -> Result<(), String> {
    for kind in Secret::ALL {
        if let Some(password) = password(from.clone(), kind).await? {
            set_password(to.clone(), kind, title.clone(), password).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saves, reads and forgets a password in the real store. Ignored: it
    /// needs an unlocked keyring, which on macOS is only the logged-in
    /// session's, not an SSH login's.
    #[tokio::test]
    #[ignore]
    async fn live_store() {
        let id = "mote-test-store".to_owned();
        let saved = Password::new("pa55 wörd".to_owned());
        set_password(id.clone(), Secret::Ssh, "Test".into(), saved.clone())
            .await
            .unwrap();
        // Saving again replaces the item, under a new title too.
        set_password(id.clone(), Secret::Ssh, "Renamed".into(), saved.clone())
            .await
            .unwrap();
        assert_eq!(
            password(id.clone(), Secret::Ssh).await.unwrap(),
            Some(saved)
        );
        assert_eq!(password(id.clone(), Secret::Account).await.unwrap(), None);
        delete_passwords(id.clone()).await.unwrap();
        assert_eq!(password(id, Secret::Ssh).await.unwrap(), None);
    }
}
