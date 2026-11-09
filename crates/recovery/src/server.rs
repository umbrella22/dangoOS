//! Read-only request contract shared by firmware dispatch and host tests.
use crate::protocol::{Operation, Packet, Status, PAYLOAD};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Volume {
    Data,
    System,
}
impl Volume {
    fn parse(v: u8) -> Result<Self, Status> {
        match v {
            0 => Ok(Self::Data),
            1 => Ok(Self::System),
            _ => Err(Status::Invalid),
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Request<'a> {
    Hello,
    Nor {
        offset: u32,
        length: usize,
    },
    NandPage {
        page: u32,
        raw: bool,
    },
    Mount(Volume),
    List {
        volume: Volume,
        index: u32,
        path: &'a str,
    },
    File {
        volume: Volume,
        offset: u32,
        length: usize,
        path: &'a str,
    },
    Logical {
        offset: u64,
        length: usize,
    },
    Status,
}
fn path(bytes: &[u8]) -> Result<&str, Status> {
    let s = core::str::from_utf8(bytes).map_err(|_| Status::Invalid)?;
    if !s.starts_with('/') || s.contains('\0') {
        return Err(Status::Invalid);
    }
    Ok(s)
}
impl<'a> Request<'a> {
    pub fn parse(p: &'a Packet) -> Result<Self, Status> {
        if p.status != 0 {
            return Err(Status::Invalid);
        }
        let d = &p.data;
        let op = Operation::try_from(p.operation).map_err(|_| Status::Invalid)?;
        let request = match op {
            Operation::Hello if d.is_empty() => Self::Hello,
            Operation::Status if d.is_empty() => Self::Status,
            Operation::Nor if d.len() == 6 => Self::Nor {
                offset: u32::from_le_bytes(d[..4].try_into().unwrap()),
                length: u16::from_le_bytes(d[4..].try_into().unwrap()) as usize,
            },
            Operation::NandPage if d.len() == 5 && d[4] <= 1 => Self::NandPage {
                page: u32::from_le_bytes(d[..4].try_into().unwrap()),
                raw: d[4] == 1,
            },
            Operation::Mount if d.len() == 1 => Self::Mount(Volume::parse(d[0])?),
            Operation::List if d.len() > 5 => Self::List {
                volume: Volume::parse(d[0])?,
                index: u32::from_le_bytes(d[1..5].try_into().unwrap()),
                path: path(&d[5..])?,
            },
            Operation::File if d.len() > 7 => Self::File {
                volume: Volume::parse(d[0])?,
                offset: u32::from_le_bytes(d[1..5].try_into().unwrap()),
                length: u16::from_le_bytes(d[5..7].try_into().unwrap()) as usize,
                path: path(&d[7..])?,
            },
            Operation::Logical if d.len() == 10 => Self::Logical {
                offset: u64::from_le_bytes(d[..8].try_into().unwrap()),
                length: u16::from_le_bytes(d[8..].try_into().unwrap()) as usize,
            },
            _ => return Err(Status::Invalid),
        };
        match request {
            Self::Nor { length, .. } | Self::Logical { length, .. } if length > PAYLOAD => {
                Err(Status::Invalid)
            }
            Self::File { length, .. } if length > PAYLOAD - 4 => Err(Status::Invalid),
            _ => Ok(request),
        }
    }
}
pub trait Media {
    fn execute(&mut self, request: Request<'_>) -> Result<Vec<u8>, Status>;
}
pub fn dispatch(media: &mut impl Media, p: &Packet) -> Packet {
    let result = Request::parse(p).and_then(|r| media.execute(r));
    let (status, data) = match result {
        Ok(data) if data.len() <= PAYLOAD => (Status::Ok, data),
        Ok(_) => (Status::Io, Vec::new()),
        Err(e) => (e, Vec::new()),
    };
    Packet {
        id: p.id,
        operation: p.operation,
        status: status as u8,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    struct NoEffects;
    impl Media for NoEffects {
        fn execute(&mut self, _: Request<'_>) -> Result<Vec<u8>, Status> {
            panic!("malformed or writable request reached media")
        }
    }
    #[test]
    fn rejects_mutations_invalid_ranges_and_paths_before_device_access() {
        for (operation, data) in [
            (0x20, vec![]),
            (2, vec![0; 5]),
            (3, vec![0, 0, 0, 0, 2]),
            (4, vec![2]),
            (6, vec![0, 0, 0, 0, 0, 1, 0, b'x']),
            (6, vec![0, 0, 0, 0, 0, 1, 0, b'/', 0]),
            (2, vec![0, 0, 0, 0, 1, 32]),
        ] {
            let p = Packet {
                id: 921,
                operation,
                status: 0,
                data,
            };
            let r = dispatch(&mut NoEffects, &p);
            assert_eq!(
                (r.id, r.operation, r.status),
                (921, operation, Status::Invalid as u8)
            );
        }
    }
    #[test]
    fn unicode_and_spaces_preserve_exact_file_identity() {
        let mut data = vec![0, 0xff, 0xff, 0xff, 0xff, 16, 0];
        data.extend_from_slice("/表情 动画/狐狸.bin".as_bytes());
        let p = Packet {
            id: 1,
            operation: 6,
            status: 0,
            data,
        };
        assert_eq!(
            Request::parse(&p),
            Ok(Request::File {
                volume: Volume::Data,
                offset: u32::MAX,
                length: 16,
                path: "/表情 动画/狐狸.bin"
            })
        );
    }
}
