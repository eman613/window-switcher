//! Separate read and write pipes avoid synchronous-handle contention.
//! Every endpoint is private to one owned peer; IO runs off the coordinator.
use std::{
    fs::{File, OpenOptions},
    mem::size_of,
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    thread,
    time::Instant,
};

use anyhow::{ensure, Context, Result};
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::{
            LocalFree, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, ERROR_PIPE_NOT_CONNECTED,
            HANDLE, HLOCAL,
        },
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
            PIPE_ACCESS_OUTBOUND, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
        },
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
            GetNamedPipeServerProcessId, SetNamedPipeHandleState, PIPE_NOWAIT, PIPE_READMODE_BYTE,
            PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
        },
    },
};

use super::{identity, transport::Streams, TICK};
use crate::process_metadata::ProcessIdentity;

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    fn current_user() -> Result<Self> {
        let sid = crate::utils::current_user_sid()?;
        let text = HSTRING::from(format!("D:P(A;;GRGW;;;{sid})(A;;GA;;;SY)"));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &text,
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }?;
        Ok(Self(descriptor))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        let _ = unsafe { LocalFree(Some(HLOCAL(self.0 .0))) };
    }
}

pub(super) struct PipePair {
    incoming: File,
    outgoing: File,
    connected: [bool; 2],
    disconnected: bool,
}

fn pipe_name(parent: u32, nonce: &str, direction: &str) -> Result<String> {
    ensure!(
        parent != 0 && nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "restart stage=pipe invalid identity"
    );
    Ok(format!(
        r"\\.\pipe\WindowSwitcher.Restart.{parent}.{nonce}.{direction}"
    ))
}

fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

impl PipePair {
    pub(super) fn create(parent: u32, nonce: &str) -> Result<Self> {
        let descriptor = SecurityDescriptor::current_user()?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0 .0,
            bInheritHandle: false.into(),
        };
        let create = |direction: &str, access: FILE_FLAGS_AND_ATTRIBUTES| -> Result<File> {
            let name = HSTRING::from(pipe_name(parent, nonce, direction)?);
            let pipe = unsafe {
                CreateNamedPipeW(
                    &name,
                    access | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    65536,
                    65536,
                    0,
                    Some(&attributes),
                )
            };
            if pipe.is_invalid() {
                return Err(windows::core::Error::from_win32())
                    .context("restart stage=pipe-create");
            }
            Ok(unsafe { File::from_raw_handle(pipe.0) })
        };
        Ok(Self {
            // GENERIC_WRITE is needed only to set PIPE_WAIT after connection.
            // CreateNamedPipe rejects FILE_WRITE_ATTRIBUTES in dwOpenMode, so
            // the server's read endpoint uses duplex access but never writes.
            // The client still opens this endpoint for writing only.
            incoming: create("reply", PIPE_ACCESS_DUPLEX)?,
            outgoing: create("request", PIPE_ACCESS_OUTBOUND)?,
            connected: [false; 2],
            disconnected: false,
        })
    }

    pub(super) fn connect(
        &mut self,
        expected: ProcessIdentity,
        check: impl Fn() -> Result<()>,
    ) -> Result<Streams> {
        loop {
            check()?;
            for (index, pipe) in [&self.incoming, &self.outgoing].into_iter().enumerate() {
                if !self.connected[index] {
                    self.connected[index] = connect_endpoint(pipe, expected.pid)?;
                }
            }
            if self.connected.iter().all(|connected| *connected) {
                identity::verify(expected)?;
                return Ok(Streams {
                    reader: Box::new(self.incoming.try_clone()?),
                    writer: Box::new(self.outgoing.try_clone()?),
                });
            }
            thread::sleep(TICK);
        }
    }

    pub(super) fn disconnect(&mut self) {
        if self.disconnected {
            return;
        }
        self.disconnected = true;
        for pipe in [&self.incoming, &self.outgoing] {
            // Disconnect also releases an outstanding worker ReadFile. Before
            // final acceptance the child's protocol treats EOF as an abort.
            if let Err(error) = unsafe { DisconnectNamedPipe(handle(pipe)) } {
                if error.code() != ERROR_PIPE_NOT_CONNECTED.to_hresult() {
                    debug!("restart stage=pipe-disconnect code={:#x}", error.code().0);
                }
            }
        }
    }
}

impl Drop for PipePair {
    fn drop(&mut self) {
        self.disconnect();
    }
}

fn connect_endpoint(pipe: &File, expected_pid: u32) -> Result<bool> {
    if let Err(error) = unsafe { ConnectNamedPipe(handle(pipe), None) } {
        if error.code() == ERROR_PIPE_LISTENING.to_hresult() {
            return Ok(false);
        }
        if error.code() != ERROR_PIPE_CONNECTED.to_hresult() {
            return Err(error).context("restart stage=pipe-connect");
        }
    }
    let mut actual = 0;
    if let Err(error) = unsafe { GetNamedPipeClientProcessId(handle(pipe), &mut actual) } {
        if [
            ERROR_PIPE_NOT_CONNECTED.to_hresult(),
            ERROR_PIPE_LISTENING.to_hresult(),
        ]
        .contains(&error.code())
        {
            return Ok(false);
        }
        return Err(error).context("restart stage=pipe-client");
    }
    ensure!(
        actual == expected_pid,
        "restart stage=pipe-client unexpected process"
    );
    unsafe { SetNamedPipeHandleState(handle(pipe), Some(&PIPE_WAIT), None, None) }?;
    Ok(true)
}

pub(super) fn client(parent: ProcessIdentity, nonce: &str, deadline: Instant) -> Result<Streams> {
    identity::verify(parent)?;
    let open = |direction: &str, read: bool| -> Result<File> {
        let name = pipe_name(parent.pid, nonce, direction)?;
        loop {
            ensure!(Instant::now() < deadline, "restart stage=pipe-open timeout");
            // An elevated client must never grant impersonation rights to the
            // ordinary server, even if a different process guesses an endpoint.
            match OpenOptions::new()
                .read(read)
                .write(!read)
                .custom_flags((SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION).0)
                .open(&name)
            {
                Ok(file) => {
                    let mut server = 0;
                    unsafe { GetNamedPipeServerProcessId(handle(&file), &mut server) }?;
                    ensure!(
                        server == parent.pid,
                        "restart stage=pipe-server unexpected process"
                    );
                    identity::verify(parent)?;
                    return Ok(file);
                }
                Err(error) if matches!(error.raw_os_error(), Some(2 | 231)) => thread::sleep(TICK),
                Err(error) => return Err(error).context("restart stage=pipe-open"),
            }
        }
    };
    let writer = open("reply", false)?;
    let reader = open("request", true)?;
    Ok(Streams {
        reader: Box::new(reader),
        writer: Box::new(writer),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        time::Duration,
    };

    #[test]
    fn private_endpoints_validate_peer_and_transfer_both_directions() {
        let parent = identity::current().unwrap();
        let nonce = format!(
            "{:032x}",
            unsafe { windows::Win32::System::Com::CoCreateGuid() }
                .unwrap()
                .to_u128()
        );
        let mut server = PipePair::create(parent.pid, &nonce).unwrap();
        let mut client = client(parent, &nonce, Instant::now() + Duration::from_secs(2)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut connected = server
            .connect(parent, || {
                ensure!(Instant::now() < deadline, "test connection timeout");
                Ok(())
            })
            .unwrap();
        client.writer.write_all(b"ready").unwrap();
        let mut received = [0; 5];
        connected.reader.read_exact(&mut received).unwrap();
        assert_eq!(&received, b"ready");
        connected.writer.write_all(b"start").unwrap();
        client.reader.read_exact(&mut received).unwrap();
        assert_eq!(&received, b"start");
        server.disconnect();
        assert!(client.reader.read_exact(&mut received).is_err());
    }

    #[test]
    fn endpoint_names_cannot_escape_the_fixed_namespace() {
        for nonce in [
            "",
            "../pipe",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/",
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            assert!(pipe_name(1, nonce, "request").is_err());
        }
        assert!(pipe_name(0, &"a".repeat(32), "request").is_err());
    }

    #[test]
    fn a_delayed_client_connects_after_nonblocking_listen() {
        let parent = identity::current().unwrap();
        let nonce = format!(
            "{:032x}",
            unsafe { windows::Win32::System::Com::CoCreateGuid() }
                .unwrap()
                .to_u128()
        );
        let mut server = PipePair::create(parent.pid, &nonce).unwrap();
        assert!(!connect_endpoint(&server.incoming, parent.pid).unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        let client = thread::spawn(move || {
            thread::sleep(TICK * 2);
            let mut stream = client(parent, &nonce, deadline).unwrap();
            stream.writer.write_all(b"late").unwrap();
            stream
        });
        let mut connected = server
            .connect(parent, || {
                ensure!(Instant::now() < deadline, "delayed test connection timeout");
                Ok(())
            })
            .unwrap();
        let mut bytes = [0; 4];
        connected.reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"late");
        drop(client.join().unwrap());
    }

    #[test]
    fn an_unexpected_client_process_cannot_complete_the_connection() {
        let parent = identity::current().unwrap();
        let nonce = format!(
            "{:032x}",
            unsafe { windows::Win32::System::Com::CoCreateGuid() }
                .unwrap()
                .to_u128()
        );
        let mut server = PipePair::create(parent.pid, &nonce).unwrap();
        let _client = client(parent, &nonce, Instant::now() + Duration::from_secs(2)).unwrap();
        let wrong_peer = ProcessIdentity { pid: 0, ..parent };
        assert!(server
            .connect(wrong_peer, || Ok(()))
            .err()
            .unwrap()
            .to_string()
            .contains("unexpected process"));
    }
}
