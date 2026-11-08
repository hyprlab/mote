//! What the window says when a session ends: a title and a sentence that
//! names the likely cause on a Linux server and where to fix it.

use crate::connection::{Connection, Protocol};
use crate::i18n::{i18n, i18n_f};
use crate::remote::{Disconnect, Ending, Failure, Security, Tunnel, Unreachable};

pub(super) enum Shown {
    /// Nothing: the window is closing.
    Nothing,
    /// Ask for the account again, saying why.
    SignIn(String),
    Page {
        title: String,
        description: String,
        /// Offer the advice on H.264.
        help: bool,
        /// Offer to go without encryption.
        allow: bool,
    },
}

/// `offered_h264`: the session offered the server H.264, so a server that
/// needs it and ends the session lacks it itself.
pub(super) fn shown(
    ending: Ending,
    connection: &Connection,
    was_connected: bool,
    offered_h264: bool,
) -> Shown {
    let host = connection.host.as_str();
    let page = |title: String, description: String| Shown::Page {
        title,
        description,
        help: false,
        allow: false,
    };
    let could_not = || i18n("Could Not Connect");
    let named = |text: &str| i18n_f(text, &[("host", &host)]);
    match ending {
        Ending::Closed => Shown::Nothing,
        Ending::Failed(failure) => match failure {
            Failure::Refused if connection.macos => Shown::SignIn(i18n(
                "The user name or password was not accepted. A Mac takes those of an account on it, with Screen Sharing or Remote Management allowed for that account.",
            )),
            Failure::Refused if connection.protocol == Protocol::Vnc => Shown::SignIn(i18n(
                "The password was not accepted. VNC servers keep a password of their own, not the login password, and VNC Authentication reads only its first 8 characters.",
            )),
            Failure::Refused => Shown::SignIn(i18n(
                "The user name or password was not accepted. GNOME Remote Desktop takes the ones set in its Remote Desktop settings, not the login password.",
            )),
            Failure::Declined(reason) => page(
                could_not(),
                i18n_f(
                    "{host} turned the connection down: “{reason}”",
                    &[("host", &host), ("reason", &reason)],
                ),
            ),
            Failure::NotAllowed => page(
                could_not(),
                named("This account is not allowed to connect to {host}."),
            ),
            Failure::Untrusted => page(
                i18n("Not Connected"),
                i18n("The computer’s identity was not trusted."),
            ),
            Failure::Unreachable(why) => page(could_not(), unreachable(why, host)),
            Failure::Addresses(tried) => page(
                could_not(),
                tried
                    .into_iter()
                    .map(|(address, failure)| missed(failure, connection, &address))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Failure::Security(Security::Unencrypted) => Shown::Page {
                title: could_not(),
                description: named(
                    "{host} offers no encryption, so the password and the session would cross the network as they are. Turn encryption on there: TigerVNC takes RA2 or TLSVnc in its SecurityTypes, wayvnc an RSA key or a certificate.",
                ),
                help: false,
                allow: true,
            },
            Failure::Security(security) => page(
                could_not(),
                match security {
                    Security::NoCertificate => named(
                        "{host} has no certificate for secure connections. Set one up in its remote desktop settings.",
                    ),
                    Security::LegacyOnly => named(
                        "{host} only offers the old RDP security, which is not safe. On xrdp, check that xrdp.ini sets security_layer to tls or negotiate, and that its certificate and key files exist.",
                    ),
                    Security::OldTls => named(
                        "{host} only offers an outdated version of TLS. Update its remote desktop server, or allow TLS 1.2 or later.",
                    ),
                    Security::NoOpenSsl => named(
                        "{host} encrypts with anonymous TLS, which needs OpenSSL 3. Install the openssl-libs or libssl3 package.",
                    ),
                    // Shown with its button above; this is never reached.
                    Security::Unencrypted => named("{host} offers no encryption."),
                    Security::Unsupported(kinds) => i18n_f(
                        "{host} asks for security this version can’t use yet: {kinds}.",
                        &[("host", &host), ("kinds", &kinds)],
                    ),
                    Security::Other(code) => {
                        tracing::warn!("negotiation failed: {code}");
                        named("{host} and this computer have no security setting in common.")
                    }
                },
            ),
            Failure::Rejected => page(
                could_not(),
                named(
                    "{host} ended the connection as it began, without saying why. KRdp does this when the user name or password is wrong.",
                ),
            ),
            failure @ (Failure::Ssh(_) | Failure::Gateway(_)) => {
                page(could_not(), missed(failure, connection, host))
            }
            Failure::Lost(error) => {
                tracing::warn!("connection lost: {error}");
                page(
                    i18n("Disconnected"),
                    i18n("The connection to the computer was lost."),
                )
            }
            Failure::Protocol(error) => {
                tracing::warn!("session failed: {error}");
                let title = if was_connected {
                    i18n("Disconnected")
                } else {
                    could_not()
                };
                page(title, error)
            }
        },
        Ending::ServerClosed(Some(Disconnect::Graphics)) if offered_h264 => Shown::Page {
            title: could_not(),
            description: named(
                "{host} can only send its picture as H.264 video, and has no H.264 encoder installed.",
            ),
            help: true,
            allow: false,
        },
        Ending::ServerClosed(Some(Disconnect::Graphics)) => page(
            could_not(),
            named(
                "{host} can only send its picture as H.264 video, and this computer has no H.264 decoder. Download Cisco’s OpenH264 in Settings, under Video.",
            ),
        ),
        Ending::ServerClosed(Some(Disconnect::FreshCredentials)) => {
            Shown::SignIn(i18n("The computer asked for the password again."))
        }
        Ending::ServerClosed(reason) => page(
            i18n("Disconnected"),
            match reason {
                Some(Disconnect::LoggedOff) => i18n("The remote session was signed out."),
                Some(Disconnect::Idle) => i18n("The session was idle for too long."),
                Some(Disconnect::LogonTimeout) => {
                    i18n("The login screen timed out before anyone signed in.")
                }
                Some(Disconnect::TakenOver) => {
                    named("Another connection to {host} took the session over.")
                }
                Some(Disconnect::Denied) => named("{host} turned the connection down."),
                Some(Disconnect::ByAdministrator) => named("Someone on {host} ended the session."),
                Some(Disconnect::Shutdown) => named("{host} is shutting down or restarting."),
                Some(Disconnect::Licensing(text) | Disconnect::Other(text)) => text,
                Some(Disconnect::FreshCredentials | Disconnect::Graphics) | None => {
                    i18n("The computer ended the session.")
                }
            },
        ),
    }
}

/// Why the computer couldn't be reached at this address.
fn unreachable(why: Unreachable, host: &str) -> String {
    let named = |text: &str| i18n_f(text, &[("host", &host)]);
    match why {
        Unreachable::NotFound => named("{host} could not be found."),
        Unreachable::Refused => named("{host} is not accepting remote desktop connections."),
        Unreachable::NoAnswer => named("{host} did not answer."),
        Unreachable::Closed => named(
            "{host} closed the connection at once. If it uses GNOME Remote Login, check that a user name and password are set, and restart its remote desktop service after changing them.",
        ),
        Unreachable::NotServer => {
            named("{host} answered, but not as a remote desktop server. Check the port.")
        }
        Unreachable::Other(error) => {
            tracing::warn!("connecting: {error}");
            named("{host} could not be reached.")
        }
    }
}

/// Why this address led nowhere: the failures that go on to the next one.
fn missed(failure: Failure, connection: &Connection, host: &str) -> String {
    match failure {
        Failure::Unreachable(why) => unreachable(why, host),
        Failure::Ssh(why) => {
            let server = connection.ssh_server().unwrap_or_default().0;
            ssh(why, &server, host)
        }
        Failure::Gateway(why) => {
            let gateway = connection.gateway_server().unwrap_or_default().0;
            gateway_said(why, &gateway, host)
        }
        _ => unreachable(Unreachable::Other(String::new()), host),
    }
}

/// Why the SSH server didn't lead on to the computer.
fn ssh(why: Tunnel, server: &str, host: &str) -> String {
    let named = |text: &str| i18n_f(text, &[("server", &server), ("host", &host)]);
    match why {
        Tunnel::Unreachable(Unreachable::NotFound) => named("{server} could not be found."),
        Tunnel::Unreachable(Unreachable::Refused) => {
            named("{server} is not accepting SSH connections.")
        }
        Tunnel::Unreachable(Unreachable::NoAnswer) => named("{server} did not answer."),
        Tunnel::Unreachable(_) => named("{server} could not be reached."),
        Tunnel::Refused => named(
            "{server} did not accept the SSH agent’s keys, the keys in ~/.ssh, or the tunnel’s password.",
        ),
        Tunnel::KeyChanged => named(
            "{server} presented a different key from the one in ~/.ssh/known_hosts. Someone may be intercepting the connection. If the server was set up again, remove its old key with ssh-keygen -R.",
        ),
        Tunnel::Denied => named("{server} could not reach {host}."),
        Tunnel::NoRoute(error) => {
            tracing::warn!("SSH tunnel: {error}");
            named("{server} could not reach {host}.")
        }
        Tunnel::Other(error) => {
            tracing::warn!("SSH tunnel: {error}");
            named("The SSH tunnel through {server} could not be opened.")
        }
    }
}

/// Why the RD Gateway didn't lead on to the computer.
fn gateway_said(why: Tunnel, gateway: &str, host: &str) -> String {
    let named = |text: &str| i18n_f(text, &[("gateway", &gateway), ("host", &host)]);
    match why {
        Tunnel::Unreachable(Unreachable::NotFound) => named("{gateway} could not be found."),
        Tunnel::Unreachable(Unreachable::NoAnswer) => named("{gateway} did not answer."),
        Tunnel::Unreachable(_) => named("{gateway} could not be reached."),
        Tunnel::Refused | Tunnel::KeyChanged => {
            named("The RD Gateway {gateway} did not accept the user name or password.")
        }
        Tunnel::Denied => named("The RD Gateway {gateway} does not allow connections to {host}."),
        Tunnel::NoRoute(error) => {
            tracing::warn!("RD Gateway: {error}");
            named("The RD Gateway {gateway} could not reach {host}.")
        }
        Tunnel::Other(error) => {
            tracing::warn!("RD Gateway: {error}");
            named("The connection through the RD Gateway {gateway} failed.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_address_tried_gets_its_own_reason() {
        let connection = Connection {
            host: "box.lan".into(),
            ..Connection::default()
        };
        let failure = Failure::Addresses(vec![
            (
                "box.lan".into(),
                Failure::Unreachable(Unreachable::NotFound),
            ),
            (
                "box.vpn:3390".into(),
                Failure::Unreachable(Unreachable::NoAnswer),
            ),
        ]);
        let Shown::Page { description, .. } =
            shown(Ending::Failed(failure), &connection, false, false)
        else {
            panic!("a page says why");
        };
        assert_eq!(
            description,
            "box.lan could not be found.\nbox.vpn:3390 did not answer."
        );
    }

    #[test]
    fn a_server_needing_h264_points_to_the_advice_only_when_offered_it() {
        let connection = Connection {
            host: "kde".into(),
            ..Connection::default()
        };
        let graphics = || Ending::ServerClosed(Some(Disconnect::Graphics));
        let help = |offered| match shown(graphics(), &connection, false, offered) {
            Shown::Page { help, .. } => help,
            _ => panic!("a page"),
        };
        assert!(help(true), "the server has no encoder");
        assert!(!help(false), "this computer has no decoder");
    }
}
