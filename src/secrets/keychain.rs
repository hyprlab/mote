//! The macOS Keychain. Its calls block, and the first read by a new build
//! of the app asks the user, so each runs on a blocking thread.

use security_framework::passwords::{
    PasswordOptions, delete_generic_password_options, generic_password,
    set_generic_password_options,
};
use zeroize::Zeroizing;

use super::{KEYRING_APP, Secret};

/// `errSecItemNotFound`.
const NOT_FOUND: i32 = -25300;

/// Items are generic passwords of the app's service, each named by the
/// kind of password and the connection's id.
fn options(id: &str, kind: Secret) -> PasswordOptions {
    PasswordOptions::new_generic_password(KEYRING_APP, &format!("{}:{id}", kind.attribute()))
}

async fn blocking<T: Send + 'static>(
    call: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(call)
        .await
        .map_err(|e| e.to_string())?
}

/// The saved password's bytes, or `None` when there is none.
pub(super) async fn get(id: &str, kind: Secret) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    let id = id.to_owned();
    blocking(move || match generic_password(options(&id, kind)) {
        Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
        Err(e) if e.code() == NOT_FOUND => Ok(None),
        Err(e) => Err(e.to_string()),
    })
    .await
}

pub(super) async fn set(id: &str, kind: Secret, label: &str, password: &str) -> Result<(), String> {
    // An update would look the item up by its label too, which a renamed
    // connection no longer matches, so the item is replaced instead.
    delete(id, kind).await?;
    let (id, label) = (id.to_owned(), label.to_owned());
    let password = Zeroizing::new(password.as_bytes().to_vec());
    blocking(move || {
        let mut options = options(&id, kind);
        options.set_label(&label);
        set_generic_password_options(&password, options).map_err(|e| e.to_string())
    })
    .await
}

/// Forget the password, if there is one.
pub(super) async fn delete(id: &str, kind: Secret) -> Result<(), String> {
    let id = id.to_owned();
    blocking(
        move || match delete_generic_password_options(options(&id, kind)) {
            Err(e) if e.code() != NOT_FOUND => Err(e.to_string()),
            _ => Ok(()),
        },
    )
    .await
}
