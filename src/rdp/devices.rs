//! A folder lent to the remote desktop (MS-RDPEFS). IronRDP's backend for
//! Unix serves it; it resolves every path the server sends beneath the
//! folder and follows no symbolic links, so the server reaches nothing
//! else. Of the Linux servers only xrdp takes it; none takes a printer.

use std::collections::HashMap;
use std::os::fd::{AsFd as _, OwnedFd};
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, open, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstatat};
use nix::unistd::{UnlinkatFlags, unlinkat};

/// A file's device and inode, which tell it from another put in its place.
/// `dev_t` is 64 bits on Linux and 32 on macOS.
type Identity = (nix::libc::dev_t, nix::libc::ino_t);

use ironrdp_core::impl_as_any;
use ironrdp_pdu::PduResult;
use ironrdp_rdpdr::pdu::RdpdrPdu;
use ironrdp_rdpdr::pdu::efs::{
    AnyIoCtlCode, ClientDriveQueryDirectoryResponse, DecodedDeviceControlRequest, DesiredAccess,
    DeviceControlRequest, DeviceIoRequest, DeviceIoResponse, FileAttributes,
    FileDirectoryInformation, FileFullDirectoryInformation, FileInformationClass,
    FileInformationClassLevel, FileNamesInformation, NtStatus, PrinterIoRequest,
    ServerDeviceAnnounceResponse, ServerDriveIoRequest,
};
use ironrdp_rdpdr::pdu::esc::{ScardCall, ScardIoCtlCode};
use ironrdp_rdpdr::pdu::esp::PortIoRequest;
use ironrdp_rdpdr::{Rdpdr, RdpdrBackend};
use ironrdp_rdpdr_native::backend::NixRdpdrBackend;
use ironrdp_svc::SvcMessage;

/// The shared folder's device.
const FOLDER_ID: u32 = 1;

/// The device channel, lending `folder`.
pub(super) fn channel(folder: &Path, computer: &str) -> Rdpdr {
    let base = folder.to_string_lossy().into_owned();
    let backend = Folder {
        inner: NixRdpdrBackend::new(base.clone()),
        base: base.into(),
        paths: HashMap::new(),
        marked: HashMap::new(),
    };
    Rdpdr::new(Box::new(backend), computer.to_owned())
        .with_drives(Some(vec![(FOLDER_ID, drive_name(folder))]))
}

/// The name the server shows the folder by: its own name.
fn drive_name(folder: &Path) -> String {
    folder
        .file_name()
        .map(|n| n.to_string_lossy().replace('\0', ""))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Home".to_owned())
}

/// IronRDP's backend for Unix, made to work with xrdp:
///
/// - It lists directories only as FileBothDirectoryInformation; xrdp asks
///   for FileDirectoryInformation and got an error for every folder. The
///   backend is asked for the one it knows, and the entry is passed on in
///   the class the server wanted.
/// - It deletes a file once the last handle to it closes, as Windows does.
///   xrdp leaves the handle it made a folder with open, so a folder it
///   removed stayed until the session ended. Here, as in FreeRDP, the entry
///   goes when the handle that marked it closes.
#[derive(Debug)]
struct Folder {
    inner: NixRdpdrBackend,
    base: PathBuf,
    /// The path each open handle was opened with.
    paths: HashMap<u32, String>,
    /// Handles that marked their file for deletion, with the file's identity
    /// then, so a file made in its place since isn't the one removed.
    marked: HashMap<u32, Identity>,
}

impl_as_any!(Folder);

impl RdpdrBackend for Folder {
    fn supports_drive_security(&self) -> bool {
        self.inner.supports_drive_security()
    }

    fn reset(&mut self) -> PduResult<()> {
        self.inner.reset()
    }

    fn restore_drive(&mut self, device_id: u32) -> PduResult<()> {
        self.inner.restore_drive(device_id)
    }

    fn add_drive(&mut self, device_id: u32) -> PduResult<()> {
        self.inner.add_drive(device_id)
    }

    fn remove_drive(&mut self, device_id: u32) -> PduResult<Vec<SvcMessage>> {
        self.inner.remove_drive(device_id)
    }

    fn handle_server_device_announce_response(
        &mut self,
        pdu: ServerDeviceAnnounceResponse,
    ) -> PduResult<()> {
        self.inner.handle_server_device_announce_response(pdu)
    }

    fn handle_scard_call(
        &mut self,
        req: DeviceControlRequest<ScardIoCtlCode>,
        call: ScardCall,
    ) -> PduResult<Vec<SvcMessage>> {
        self.inner.handle_scard_call(req, call)
    }

    fn handle_drive_io_request(&mut self, req: ServerDriveIoRequest) -> PduResult<Vec<SvcMessage>> {
        let req = as_xrdp_means(req);
        match &req {
            ServerDriveIoRequest::ServerCreateDriveRequest(create) => {
                let path = create.path.clone();
                let replies = self.inner.handle_drive_io_request(req)?;
                if let Some(file_id) = replies.first().and_then(created_file_id) {
                    self.paths.insert(file_id, path);
                }
                return Ok(replies);
            }
            ServerDriveIoRequest::ServerDriveSetInformationRequest(set) => {
                if let FileInformationClass::Disposition(disposition) = &set.set_buffer {
                    let file_id = set.device_io_request.file_id;
                    let identity = self.paths.get(&file_id).and_then(|p| self.identity(p));
                    match identity.filter(|_| disposition.delete_pending != 0) {
                        Some(identity) => self.marked.insert(file_id, identity),
                        None => self.marked.remove(&file_id),
                    };
                }
                return self.inner.handle_drive_io_request(req);
            }
            ServerDriveIoRequest::DeviceCloseRequest(close) => {
                let file_id = close.device_io_request.file_id;
                let replies = self.inner.handle_drive_io_request(req)?;
                let path = self.paths.remove(&file_id);
                if let (Some(identity), Some(path)) = (self.marked.remove(&file_id), path) {
                    self.remove(&path, identity);
                }
                return Ok(replies);
            }
            _ => {}
        }
        let ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(mut query) = req else {
            return self.inner.handle_drive_io_request(req);
        };
        let wanted = query.file_info_class_lvl.clone();
        let other = [
            FileInformationClassLevel::FILE_DIRECTORY_INFORMATION,
            FileInformationClassLevel::FILE_FULL_DIRECTORY_INFORMATION,
            FileInformationClassLevel::FILE_NAMES_INFORMATION,
        ];
        if !other.contains(&wanted) {
            return self.inner.handle_drive_io_request(
                ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(query),
            );
        }
        let request = query.device_io_request.clone();
        query.file_info_class_lvl = FileInformationClassLevel::FILE_BOTH_DIRECTORY_INFORMATION;
        let replies = self.inner.handle_drive_io_request(
            ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(query),
        )?;
        Ok(replies
            .into_iter()
            .map(|reply| relisted(reply, &request, &wanted))
            .collect())
    }

    fn handle_drive_device_control(
        &mut self,
        req: DecodedDeviceControlRequest<AnyIoCtlCode>,
    ) -> PduResult<Vec<SvcMessage>> {
        self.inner.handle_drive_device_control(req)
    }

    fn poll_deferred_messages(&mut self) -> PduResult<Vec<SvcMessage>> {
        self.inner.poll_deferred_messages()
    }

    fn handle_user_logged_on(&mut self, rdpdr: &mut Rdpdr) -> PduResult<Vec<SvcMessage>> {
        self.inner.handle_user_logged_on(rdpdr)
    }

    fn handle_printer_io_request(&mut self, req: PrinterIoRequest) -> PduResult<Vec<SvcMessage>> {
        self.inner.handle_printer_io_request(req)
    }

    fn handle_port_io_request(&mut self, req: PortIoRequest) -> PduResult<Vec<SvcMessage>> {
        self.inner.handle_port_io_request(req)
    }

    fn reject_printer_write(&mut self, req: DeviceIoRequest) -> PduResult<Vec<SvcMessage>> {
        self.inner.reject_printer_write(req)
    }
}

impl Folder {
    /// The parent directory of a path the server sent, reached without
    /// following a symbolic link, and the name in it. `None` for a path that
    /// would climb out, which the backend has turned down already.
    fn parent<'a>(&self, path: &'a str) -> Option<(OwnedFd, &'a str)> {
        let parts: Vec<&str> = path.split('\\').filter(|p| !p.is_empty()).collect();
        let (name, parents) = parts.split_last()?;
        if parts.iter().any(|p| matches!(*p, "." | "..")) {
            return None;
        }
        let flags = OFlag::O_DIRECTORY | OFlag::O_RDONLY | OFlag::O_CLOEXEC;
        let mut dir = open(self.base.as_path(), flags, Mode::empty()).ok()?;
        for part in parents {
            dir = openat(dir.as_fd(), *part, flags | OFlag::O_NOFOLLOW, Mode::empty()).ok()?;
        }
        Some((dir, name))
    }

    fn stat(&self, path: &str) -> Option<FileStat> {
        let (parent, name) = self.parent(path)?;
        fstatat(parent.as_fd(), name, AtFlags::AT_SYMLINK_NOFOLLOW).ok()
    }

    fn identity(&self, path: &str) -> Option<Identity> {
        self.stat(path).map(|s| (s.st_dev, s.st_ino))
    }

    /// Remove the entry at `path` if it is still the one marked. One the
    /// backend removed already is gone, and that is fine.
    fn remove(&self, path: &str, identity: Identity) {
        let (Some((parent, name)), Some(stat)) = (self.parent(path), self.stat(path)) else {
            return;
        };
        if (stat.st_dev, stat.st_ino) != identity {
            return;
        }
        let directory = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR;
        let flags = if directory {
            UnlinkatFlags::RemoveDir
        } else {
            UnlinkatFlags::NoRemoveDir
        };
        match unlinkat(parent.as_fd(), name, flags) {
            Ok(()) | Err(Errno::ENOENT) => {}
            Err(e) => tracing::debug!("removing {path} from the shared folder: {e}"),
        }
    }
}

/// The handle a create reply gives the server: after the RDPDR header (4
/// bytes) and the I/O response (12), whose status must be success.
fn created_file_id(reply: &SvcMessage) -> Option<u32> {
    let bytes = reply.encode_unframed_pdu().ok()?;
    let u32_at = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    (u32_at(12)? == 0).then(|| u32_at(16)).flatten()
}

/// The request as xrdp means it, which IronRDP's backend reads more
/// strictly than Windows does:
///
/// - xrdp writes some paths with forward slashes, which the backend turns
///   down as invalid; they get Windows' backslashes.
/// - xrdp deletes a file through a handle opened only to read its
///   attributes, where the backend wants the handle to carry DELETE access.
///   Every open carries it: a server could ask for it anyway, so this lends
///   it nothing new.
fn as_xrdp_means(mut req: ServerDriveIoRequest) -> ServerDriveIoRequest {
    let fix = |path: &mut String| {
        if path.contains('/') {
            *path = path.replace('/', "\\");
        }
    };
    match &mut req {
        ServerDriveIoRequest::ServerCreateDriveRequest(create) => {
            fix(&mut create.path);
            create.desired_access |= DesiredAccess::DELETE;
        }
        ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(query) => fix(&mut query.path),
        ServerDriveIoRequest::ServerDriveSetInformationRequest(set) => {
            if let FileInformationClass::Rename(rename) = &mut set.set_buffer {
                fix(&mut rename.file_name);
            }
        }
        _ => {}
    }
    req
}

/// A listing reply in the class the server asked for. A failure carries no
/// entry and goes as it is.
fn relisted(
    reply: SvcMessage,
    request: &DeviceIoRequest,
    class: &FileInformationClassLevel,
) -> SvcMessage {
    let Some(entry) = reply
        .encode_unframed_pdu()
        .ok()
        .and_then(|bytes| both_entry(&bytes))
    else {
        return reply;
    };
    let buffer = match *class {
        FileInformationClassLevel::FILE_NAMES_INFORMATION => {
            FileInformationClass::Names(FileNamesInformation::new(entry.name))
        }
        FileInformationClassLevel::FILE_FULL_DIRECTORY_INFORMATION => {
            FileInformationClass::FullDirectory(FileFullDirectoryInformation::new(
                entry.created,
                entry.accessed,
                entry.written,
                entry.changed,
                entry.size,
                entry.attributes,
                entry.name,
            ))
        }
        _ => FileInformationClass::Directory(FileDirectoryInformation::new(
            entry.created,
            entry.accessed,
            entry.written,
            entry.changed,
            entry.size,
            entry.attributes,
            entry.name,
        )),
    };
    SvcMessage::from(RdpdrPdu::ClientDriveQueryDirectoryResponse(
        ClientDriveQueryDirectoryResponse {
            device_io_reply: DeviceIoResponse::new(request.clone(), NtStatus::SUCCESS),
            buffer: Some(buffer),
        },
    ))
}

struct Entry {
    created: i64,
    accessed: i64,
    written: i64,
    changed: i64,
    size: i64,
    attributes: FileAttributes,
    name: String,
}

/// The entry in a successful FileBothDirectoryInformation reply: the RDPDR
/// header (4 bytes), the I/O response (12), the length (4), then the entry
/// ([MS-FSCC] 2.4.8), whose name starts after 93 bytes as IronRDP writes it.
fn both_entry(bytes: &[u8]) -> Option<Entry> {
    let u32_at = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let i64_at = |at: usize| Some(i64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?));
    if u32_at(12)? != 0 {
        return None;
    }
    let entry = 20;
    let name_length = u32_at(entry + 60)? as usize;
    let name = bytes.get(entry + 93..entry + 93 + name_length)?;
    let units: Vec<u16> = name
        .as_chunks::<2>()
        .0
        .iter()
        .map(|u| u16::from_le_bytes(*u))
        .collect();
    Some(Entry {
        created: i64_at(entry + 8)?,
        accessed: i64_at(entry + 16)?,
        written: i64_at(entry + 24)?,
        changed: i64_at(entry + 32)?,
        size: i64_at(entry + 40)?,
        attributes: FileAttributes::from_bits_retain(u32_at(entry + 56)?),
        name: String::from_utf16_lossy(&units),
    })
}

#[cfg(test)]
mod tests {
    use ironrdp_rdpdr::pdu::efs::FileBothDirectoryInformation;

    use super::*;

    #[test]
    fn a_listing_entry_is_read_back() {
        let request = DeviceIoRequest {
            device_id: 1,
            file_id: 7,
            completion_id: 9,
            major_function: ironrdp_rdpdr::pdu::efs::MajorFunction::DirectoryControl,
            minor_function: ironrdp_rdpdr::pdu::efs::MinorFunction::from(0x01),
        };
        let both = SvcMessage::from(RdpdrPdu::ClientDriveQueryDirectoryResponse(
            ClientDriveQueryDirectoryResponse {
                device_io_reply: DeviceIoResponse::new(request.clone(), NtStatus::SUCCESS),
                buffer: Some(FileInformationClass::BothDirectory(
                    FileBothDirectoryInformation::new(
                        1,
                        2,
                        3,
                        4,
                        5000,
                        FileAttributes::FILE_ATTRIBUTE_DIRECTORY,
                        "Fotos ✓".to_owned(),
                    ),
                )),
            },
        ));
        let entry = both_entry(&both.encode_unframed_pdu().unwrap()).expect("an entry");
        assert_eq!(
            (
                entry.created,
                entry.accessed,
                entry.written,
                entry.changed,
                entry.size
            ),
            (1, 2, 3, 4, 5000)
        );
        assert_eq!(entry.attributes, FileAttributes::FILE_ATTRIBUTE_DIRECTORY);
        assert_eq!(entry.name, "Fotos ✓");
    }
}
