//! fxdax's own: how an object holding extents' data is framed (backlog doc-7, Segments): its data,
//! then its footer, then a CRC32C of each [`PART`] of the data, then a [`TRAILER`] of the lengths of
//! the data and the footer. A store's closed segments are framed so, as are the objects a branch
//! keeps in object storage, whose data a host fetches by parts, each checked against its CRC.
//!
//! A branch's checkpoints, locally and in object storage, are such objects: tables of where its
//! extents are ([`table`]).

pub mod table;

/// Each this many bytes of an object's data, the last fewer, is a part, with its own CRC32C: what a
/// host fetches at once, and checks.
pub const PART: u64 = 1 << 20;

/// The bytes after an object's CRCs: the lengths of its data and its footer.
pub const TRAILER: u64 = 16;

/// The CRC32C of `bytes`.
pub fn crc(bytes: &[u8]) -> u32 {
    crc_fast::checksum(crc_fast::CrcAlgorithm::Crc32Iscsi, bytes) as u32
}

/// The CRC32Cs of `data`'s parts.
pub fn part_crcs(data: &[u8]) -> Vec<u32> {
    data.chunks(PART as usize).map(crc).collect()
}

/// What follows an object's data, of `data_len` bytes whose parts' CRCs are `crcs`: `footer`, the
/// CRCs, and the trailer.
pub fn tail(data_len: u64, crcs: &[u32], footer: &[u8]) -> Vec<u8> {
    let mut bytes = footer.to_vec();
    for crc in crcs {
        bytes.extend_from_slice(&crc.to_le_bytes());
    }
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.extend_from_slice(&(footer.len() as u64).to_le_bytes());
    bytes
}

/// The bytes of an object of `data` and `footer`.
pub fn framed(data: &[u8], footer: &[u8]) -> Vec<u8> {
    let mut bytes = data.to_vec();
    bytes.extend_from_slice(&tail(data.len() as u64, &part_crcs(data), footer));
    bytes
}

/// An object that is not as its frame says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Corrupt(pub String);

impl std::fmt::Display for Corrupt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Corrupt {}

fn short(what: &str) -> Corrupt {
    Corrupt(format!("an object too short for {what}"))
}

/// Where the pieces of an object [`framed`] are, as its trailer says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub data_len: u64,
    pub footer_len: u64,
}

impl Frame {
    /// The frame an object's last bytes, its [`TRAILER`] at their end, give.
    pub fn of_trailer(bytes: &[u8]) -> Result<Self, Corrupt> {
        let at = bytes.len().checked_sub(TRAILER as usize).ok_or_else(|| short("its trailer"))?;
        let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        let frame = Self { data_len: u64_at(at), footer_len: u64_at(at + 8) };
        // Lengths so large that the object's would overflow are corrupt.
        frame.data_len.checked_add(frame.footer_len).ok_or_else(|| short("its lengths"))?;
        Ok(frame)
    }

    /// The frame of a whole object.
    pub fn of(bytes: &[u8]) -> Result<Self, Corrupt> {
        let frame = Self::of_trailer(bytes)?;
        if frame.size() != bytes.len() as u64 {
            return Err(short("what its trailer says"));
        }
        Ok(frame)
    }

    pub fn parts(&self) -> u64 {
        self.data_len.div_ceil(PART)
    }

    /// Where part `part` of the data is.
    pub fn part(&self, part: u64) -> std::ops::Range<u64> {
        (part * PART).min(self.data_len)..((part + 1) * PART).min(self.data_len)
    }

    pub fn footer(&self) -> std::ops::Range<u64> {
        self.data_len..self.data_len + self.footer_len
    }

    pub fn crcs(&self) -> std::ops::Range<u64> {
        let at = self.footer().end;
        at..at.saturating_add(4 * self.parts())
    }

    /// The object's length.
    pub fn size(&self) -> u64 {
        self.crcs().end.saturating_add(TRAILER)
    }

    /// The CRCs in `suffix`, the object's last bytes, which must hold them.
    pub fn crcs_in(&self, suffix: &[u8]) -> Result<Vec<u32>, Corrupt> {
        let at = (suffix.len() as u64 + self.crcs().start)
            .checked_sub(self.size())
            .ok_or_else(|| short("its CRCs"))? as usize;
        let crcs = &suffix[at..suffix.len() - TRAILER as usize];
        Ok(crcs.as_chunks::<4>().0.iter().map(|crc| u32::from_le_bytes(*crc)).collect())
    }
}

/// The data of a whole object [`framed`], checked against its CRCs.
pub fn data(bytes: &[u8]) -> Result<&[u8], Corrupt> {
    let frame = Frame::of(bytes)?;
    let data = &bytes[..frame.data_len as usize];
    let crcs = frame.crcs_in(bytes)?;
    if part_crcs(data) != crcs {
        return Err(Corrupt("an object's data, by its CRCs".into()));
    }
    Ok(data)
}

/// The footer of a whole object [`framed`].
pub fn footer(bytes: &[u8]) -> Result<&[u8], Corrupt> {
    let footer = Frame::of(bytes)?.footer();
    Ok(&bytes[footer.start as usize..footer.end as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Objects carry a CRC32C for each part of their data, which a ranged read of their end finds.
    #[test]
    fn objects_hold_their_parts_crcs() {
        let contents: Vec<u8> = (0..PART + 7).map(|i| (i % 251) as u8).collect();
        let bytes = framed(&contents, b"footer");
        let frame = Frame::of(&bytes).unwrap();
        assert_eq!(frame, Frame { data_len: PART + 7, footer_len: 6 });
        assert_eq!(frame.parts(), 2);
        assert_eq!(frame.part(1), PART..PART + 7);
        assert_eq!(frame.size(), bytes.len() as u64);
        let suffix = &bytes[frame.crcs().start as usize - 3..];
        let crcs = vec![crc(&contents[..PART as usize]), crc(&contents[PART as usize..])];
        assert_eq!(frame.crcs_in(suffix).unwrap(), crcs);
        assert_eq!(Frame::of_trailer(&bytes[bytes.len() - 16..]).unwrap(), frame);
        assert!(frame.crcs_in(&bytes[frame.crcs().start as usize + 1..]).is_err());
        assert_eq!(data(&bytes).unwrap(), &contents[..]);
        assert_eq!(footer(&bytes).unwrap(), b"footer");

        let mut flipped = bytes.clone();
        flipped[PART as usize + 3] ^= 1;
        assert_eq!(data(&flipped), Err(Corrupt("an object's data, by its CRCs".into())));
        assert!(Frame::of(&bytes[1..]).is_err());

        let empty = framed(&[], &[]);
        assert_eq!(Frame::of(&empty).unwrap(), Frame { data_len: 0, footer_len: 0 });
        assert_eq!(empty.len() as u64, TRAILER);
        // A tail made apart from the data it follows.
        let tail = tail(contents.len() as u64, &part_crcs(&contents), b"footer");
        assert_eq!([contents, tail].concat(), bytes);
    }
}
