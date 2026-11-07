//! How an RDP session's errors read as the reasons in
//! [`crate::remote::failure`]: IronRDP's connector errors, the negotiation
//! failures servers send, and the reasons they give for ending a session.

use ironrdp_connector::{ConnectorError, ConnectorErrorKind};
use ironrdp_pdu::nego::FailureCode;
use ironrdp_pdu::rdp::server_error_info::{
    ErrorInfo, ProtocolIndependentCode, ProtocolIndependentLicensingCode, RdpSpecificCode,
};

use super::{describe, io_kind};
use crate::remote::{Disconnect, Failure, Security, Unreachable};

impl Failure {
    /// A failure in the first exchange, before TLS: a server that closes
    /// the connection then turns every client away.
    pub(super) fn opening(error: &ConnectorError) -> Self {
        use std::io::ErrorKind;
        match error.kind() {
            ConnectorErrorKind::Negotiation(failure) => Self::Security(security(failure.code())),
            // xrdp falls back to Standard RDP Security, rather than refusing
            // as the spec has it, when set to it or when its certificate is
            // missing; IronRDP says so only in its message.
            ConnectorErrorKind::Reason(text) if text.contains("STANDARD_RDP_SECURITY") => {
                Self::Security(Security::LegacyOnly)
            }
            // What came back wasn't an X.224 Connection Confirm.
            ConnectorErrorKind::Decode(_) => Self::Unreachable(Unreachable::NotServer),
            // Not even framed as RDP: an SSH or VNC port, say.
            _ if undecodable(error) => Self::Unreachable(Unreachable::NotServer),
            _ => match io_kind(error) {
                Some(
                    ErrorKind::ConnectionReset | ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe,
                ) => {
                    tracing::warn!("opening: {}", describe(error));
                    Self::Unreachable(Unreachable::Closed)
                }
                _ => Self::Protocol(describe(error)),
            },
        }
    }

    /// A failed TLS handshake.
    pub(super) fn tls(error: &std::io::Error) -> Self {
        use rustls::{AlertDescription, Error};
        let tls = error.get_ref().and_then(|e| e.downcast_ref::<Error>());
        match tls {
            Some(
                Error::PeerIncompatible(_)
                | Error::AlertReceived(
                    AlertDescription::ProtocolVersion
                    | AlertDescription::HandshakeFailure
                    | AlertDescription::InsufficientSecurity,
                ),
            ) => {
                tracing::warn!("TLS: {}", describe(error));
                Self::Security(Security::OldTls)
            }
            _ => match error.kind() {
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof => {
                    tracing::warn!("TLS: {}", describe(error));
                    Self::Unreachable(Unreachable::Closed)
                }
                _ => Self::Lost(describe(error)),
            },
        }
    }

    /// A failure after TLS, while the connection is set up.
    pub(super) fn from_connector(error: &ConnectorError) -> Self {
        use std::io::ErrorKind;
        match error.kind() {
            ConnectorErrorKind::AccessDenied => Self::NotAllowed,
            ConnectorErrorKind::Credssp(_) => Self::Refused,
            ConnectorErrorKind::Negotiation(failure) => Self::Security(security(failure.code())),
            _ => {
                let text = describe(error);
                tracing::warn!("setting up: {text}");
                let closed = matches!(
                    io_kind(error),
                    Some(
                        ErrorKind::ConnectionReset
                            | ErrorKind::UnexpectedEof
                            | ErrorKind::BrokenPipe
                    )
                );
                // IronRDP reports a Disconnect Provider Ultimatum only in its
                // message.
                if closed || text.contains("disconnect provider ultimatum") {
                    Self::Rejected
                } else {
                    Self::Protocol(text)
                }
            }
        }
    }
}

/// Whether an error comes from bytes that didn't decode as RDP. The framing
/// wraps its decode errors in I/O errors, whose own chain skips them.
fn undecodable(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(e) = current {
        if e.is::<ironrdp_core::DecodeError>() {
            return true;
        }
        if let Some(inner) = e
            .downcast_ref::<std::io::Error>()
            .and_then(|io| io.get_ref())
            && inner.is::<ironrdp_core::DecodeError>()
        {
            return true;
        }
        current = e.source();
    }
    false
}

fn security(code: FailureCode) -> Security {
    if code == FailureCode::SSL_CERT_NOT_ON_SERVER {
        Security::NoCertificate
    } else if code == FailureCode::SSL_NOT_ALLOWED_BY_SERVER {
        Security::LegacyOnly
    } else {
        Security::Other(format!("{code:?}"))
    }
}

impl Disconnect {
    /// The reason in a Set Error Info PDU; `None` for "no error".
    pub(super) fn from_error_info(info: ErrorInfo) -> Option<Self> {
        use ProtocolIndependentCode as Code;
        Some(match info {
            ErrorInfo::ProtocolIndependentCode(code) => match code {
                Code::None => return None,
                Code::RpcInitiatedLogoff | Code::LogoffByUser => Self::LoggedOff,
                Code::IdleTimeout => Self::Idle,
                Code::LogonTimeout => Self::LogonTimeout,
                Code::DisconnectedByOtherconnection => Self::TakenOver,
                Code::ServerDeniedConnection | Code::ServerInsufficientPrivileges => Self::Denied,
                Code::RpcInitiatedDisconnect | Code::RpcInitiatedDisconnectByuser => {
                    Self::ByAdministrator
                }
                Code::ServerShutdown | Code::ServerReboot => Self::Shutdown,
                Code::ServerFreshCredentialsRequired => Self::FreshCredentials,
                other => Self::Other(other.description().to_owned()),
            },
            ErrorInfo::ProtocolIndependentLicensingCode(code) => {
                if code == ProtocolIndependentLicensingCode::NoRemoteConnections {
                    Self::Denied
                } else {
                    Self::Licensing(code.description().to_owned())
                }
            }
            ErrorInfo::RdpSpecificCode(RdpSpecificCode::GraphicsSubsystemFailed) => Self::Graphics,
            other => Self::Other(other.description()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_reasons_servers_give() {
        let reason = |code| Disconnect::from_error_info(ErrorInfo::ProtocolIndependentCode(code));
        assert!(reason(ProtocolIndependentCode::None).is_none());
        assert!(matches!(
            reason(ProtocolIndependentCode::DisconnectedByOtherconnection),
            Some(Disconnect::TakenOver)
        ));
        assert!(matches!(
            reason(ProtocolIndependentCode::LogoffByUser),
            Some(Disconnect::LoggedOff)
        ));
        assert!(matches!(
            reason(ProtocolIndependentCode::ServerReboot),
            Some(Disconnect::Shutdown)
        ));
    }

    #[test]
    fn reads_negotiation_failures() {
        assert!(matches!(
            security(FailureCode::SSL_CERT_NOT_ON_SERVER),
            Security::NoCertificate
        ));
        assert!(matches!(
            security(FailureCode::SSL_NOT_ALLOWED_BY_SERVER),
            Security::LegacyOnly
        ));
    }
}
