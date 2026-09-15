use std::{
    io::{Read, Write},
    sync::mpsc::{self, Receiver},
    thread,
};

use anyhow::{bail, Context, Result};

const MAGIC: &[u8; 4] = b"WSR2";
const MAX_SNAPSHOT: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Signal {
    Prepared = 1,
    Activate = 2,
    Active = 3,
    Commit = 4,
    Committed = 5,
    Abort = 6,
}

impl Signal {
    pub(super) fn read(reader: &mut impl Read) -> Result<Self> {
        let mut byte = [0];
        reader
            .read_exact(&mut byte)
            .context("restart stage=pipe-read")?;
        Ok(match byte[0] {
            1 => Self::Prepared,
            2 => Self::Activate,
            3 => Self::Active,
            4 => Self::Commit,
            5 => Self::Committed,
            6 => Self::Abort,
            _ => bail!("restart stage=protocol unknown signal"),
        })
    }

    pub(super) fn write(self, writer: &mut impl Write) -> Result<()> {
        writer.write_all(&[self as u8])?;
        writer.flush()?;
        Ok(())
    }
}

pub(super) fn write_snapshot(writer: &mut impl Write, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_SNAPSHOT {
        bail!("restart stage=snapshot invalid size");
    }
    writer.write_all(MAGIC)?;
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(bytes)?;
    writer.flush()?;
    Ok(())
}

pub(super) fn read_snapshot(reader: &mut impl Read) -> Result<Vec<u8>> {
    let mut header = [0; 8];
    reader.read_exact(&mut header)?;
    if &header[..4] != MAGIC {
        bail!("restart stage=snapshot invalid protocol");
    }
    let length = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
    if length == 0 || length > MAX_SNAPSHOT {
        bail!("restart stage=snapshot invalid size");
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Blocking pipe reads are confined to one bounded producer. Closing/killing the
/// owned peer closes the pipe; UI and coordinator threads never wait in ReadFile.
pub(super) fn reader(mut pipe: impl Read + Send + 'static) -> Result<Receiver<Result<Signal>>> {
    let (tx, rx) = mpsc::sync_channel(8);
    thread::Builder::new()
        .name("restart-pipe-read".into())
        .spawn(move || loop {
            let signal = Signal::read(&mut pipe);
            let failed = signal.is_err();
            if tx.send(signal).is_err() || failed {
                break;
            }
        })?;
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_snapshot_preserves_exact_bytes_and_rejects_unbounded_or_truncated_frames() {
        let source = b"\xff\xfe;\0 \0x\0\r\0\n\0";
        let mut wire = Vec::new();
        write_snapshot(&mut wire, source).unwrap();
        assert_eq!(read_snapshot(&mut wire.as_slice()).unwrap(), source);
        wire.pop();
        assert!(read_snapshot(&mut wire.as_slice()).is_err());
        assert!(read_snapshot(&mut b"WSR2\xff\xff\xff\xff".as_slice()).is_err());
        assert!(read_snapshot(&mut b"FAKE\x01\0\0\0x".as_slice()).is_err());
        assert!(Signal::read(&mut [0u8].as_slice()).is_err());
    }
}
