//! USB/IP 1.1.1 wire format. Lengths are validated before allocation.
use std::io::{self, IoSlice, Read, Write};

pub const VERSION: u16 = 0x0111;
pub const DEVLIST: u16 = 0x8005;
pub const IMPORT: u16 = 0x8003;
pub const SUBMIT: u32 = 1;
pub const UNLINK: u32 = 2;
pub const RET_SUBMIT: u32 = 3;
pub const RET_UNLINK: u32 = 4;
pub const MAX_TRANSFER: usize = 32 * 1024 * 1024;
pub const MAX_ISO_PACKETS: usize = 16 * 1024;

pub fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
pub fn u16be(b: &[u8]) -> u16 {
    u16::from_be_bytes(b[..2].try_into().unwrap())
}
pub fn u32be(b: &[u8]) -> u32 {
    u32::from_be_bytes(b[..4].try_into().unwrap())
}
pub fn put32(b: &mut [u8], v: u32) {
    b[..4].copy_from_slice(&v.to_be_bytes());
}
pub fn write_vectored_all(w: &mut impl Write, slices: &mut [IoSlice<'_>]) -> io::Result<()> {
    let mut remaining = slices;
    while !remaining.is_empty() {
        match w.write_vectored(remaining) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => IoSlice::advance_slices(&mut remaining, n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

pub fn fixed_string(b: &[u8]) -> io::Result<String> {
    let n = b
        .iter()
        .position(|&v| v == 0)
        .ok_or_else(|| invalid("unterminated wire string"))?;
    std::str::from_utf8(&b[..n])
        .map(str::to_owned)
        .map_err(|_| invalid("invalid UTF-8"))
}
pub fn put_string(b: &mut [u8], value: &str) -> io::Result<()> {
    if value.len() >= b.len() || value.as_bytes().contains(&0) {
        return Err(invalid("wire string too long"));
    }
    b.fill(0);
    b[..value.len()].copy_from_slice(value.as_bytes());
    Ok(())
}
pub fn operation(code: u16, status: u32) -> [u8; 8] {
    let mut b = [0; 8];
    b[..2].copy_from_slice(&VERSION.to_be_bytes());
    b[2..4].copy_from_slice(&code.to_be_bytes());
    put32(&mut b[4..], status);
    b
}
pub fn read_operation(r: &mut impl Read) -> io::Result<(u16, u32)> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    if u16be(&b) != VERSION {
        return Err(invalid("unsupported USB/IP version"));
    }
    Ok((u16be(&b[2..]), u32be(&b[4..])))
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Device {
    pub path: String,
    pub busid: String,
    pub bus: u32,
    pub address: u32,
    pub speed: u32,
    pub vendor: u16,
    pub product: u16,
    pub revision: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub configuration: u8,
    pub configurations: u8,
    pub interfaces: Vec<[u8; 4]>,
}
impl Device {
    pub fn devid(&self) -> u32 {
        (self.bus << 16) | self.address
    }
    pub fn encode(&self) -> io::Result<[u8; 312]> {
        let mut b = [0; 312];
        put_string(&mut b[..256], &self.path)?;
        put_string(&mut b[256..288], &self.busid)?;
        for (i, v) in [self.bus, self.address, self.speed].iter().enumerate() {
            put32(&mut b[288 + i * 4..], *v);
        }
        b[300..302].copy_from_slice(&self.vendor.to_be_bytes());
        b[302..304].copy_from_slice(&self.product.to_be_bytes());
        b[304..306].copy_from_slice(&self.revision.to_be_bytes());
        b[306..312].copy_from_slice(&[
            self.class,
            self.subclass,
            self.protocol,
            self.configuration,
            self.configurations,
            self.interfaces
                .len()
                .try_into()
                .map_err(|_| invalid("too many interfaces"))?,
        ]);
        Ok(b)
    }
    pub fn decode(b: &[u8; 312]) -> io::Result<Self> {
        Ok(Self {
            path: fixed_string(&b[..256])?,
            busid: fixed_string(&b[256..288])?,
            bus: u32be(&b[288..]),
            address: u32be(&b[292..]),
            speed: u32be(&b[296..]),
            vendor: u16be(&b[300..]),
            product: u16be(&b[302..]),
            revision: u16be(&b[304..]),
            class: b[306],
            subclass: b[307],
            protocol: b[308],
            configuration: b[309],
            configurations: b[310],
            interfaces: vec![[0; 4]; b[311] as usize],
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct IsoPacket {
    pub offset: u32,
    pub length: u32,
    pub actual: u32,
    pub status: i32,
}
#[derive(Debug)]
pub struct Submit {
    pub seq: u32,
    pub devid: u32,
    pub direction: u32,
    pub endpoint: u32,
    pub flags: u32,
    pub length: usize,
    pub start_frame: u32,
    pub packet_count: u32,
    pub interval: u32,
    pub setup: [u8; 8],
    pub data: Vec<u8>,
    pub iso: Vec<IsoPacket>,
}
pub enum Request {
    Submit(Submit),
    Unlink { seq: u32, target: u32 },
}
pub fn read_request(r: &mut impl Read) -> io::Result<Request> {
    let mut b = [0; 48];
    r.read_exact(&mut b)?;
    let seq = u32be(&b[4..]);
    match u32be(&b) {
        UNLINK => Ok(Request::Unlink {
            seq,
            target: u32be(&b[20..]),
        }),
        SUBMIT => {
            let length = u32be(&b[24..]) as usize;
            let direction = u32be(&b[12..]);
            let endpoint = u32be(&b[16..]);
            let packet_count = u32be(&b[32..]);
            let packets = if packet_count == u32::MAX {
                0
            } else {
                packet_count as usize
            };
            if length > MAX_TRANSFER || direction > 1 || endpoint > 15 || packets > MAX_ISO_PACKETS
            {
                return Err(invalid("invalid transfer dimensions"));
            }
            let mut data = if direction == 0 {
                vec![0; length]
            } else {
                Vec::new()
            };
            r.read_exact(&mut data)?;
            let mut iso = Vec::with_capacity(packets);
            let mut sum = 0usize;
            for _ in 0..packets {
                let mut d = [0; 16];
                r.read_exact(&mut d)?;
                let p = IsoPacket {
                    offset: u32be(&d),
                    length: u32be(&d[4..]),
                    actual: u32be(&d[8..]),
                    status: u32be(&d[12..]) as i32,
                };
                if p.offset as usize + p.length as usize > length || p.actual > p.length {
                    return Err(invalid("iso packet outside transfer"));
                }
                sum = sum
                    .checked_add(p.length as usize)
                    .ok_or_else(|| invalid("iso length overflow"))?;
                iso.push(p);
            }
            if sum > length {
                return Err(invalid("iso packets exceed buffer"));
            }
            Ok(Request::Submit(Submit {
                seq,
                devid: u32be(&b[8..]),
                direction,
                endpoint,
                flags: u32be(&b[20..]),
                length,
                start_frame: u32be(&b[28..]),
                packet_count,
                interval: u32be(&b[36..]),
                setup: b[40..48].try_into().unwrap(),
                data,
                iso,
            }))
        }
        _ => Err(invalid("unknown USB/IP transfer command")),
    }
}
impl Submit {
    pub fn encode(&self) -> [u8; 48] {
        let mut b = [0; 48];
        for (i, v) in [
            SUBMIT,
            self.seq,
            self.devid,
            self.direction,
            self.endpoint,
            self.flags,
            self.length as u32,
            self.start_frame,
            self.packet_count,
            self.interval,
        ]
        .iter()
        .enumerate()
        {
            put32(&mut b[i * 4..], *v);
        }
        b[40..].copy_from_slice(&self.setup);
        b
    }
}

pub struct Completion {
    pub header: [u8; 48],
    pub data: Vec<u8>,
    pub data_start: usize,
    pub data_len: usize,
    pub iso: Vec<IsoPacket>,
    pub memory: Option<crate::budget::MemoryLease>,
}
impl Completion {
    pub fn submit(
        req: &Submit,
        status: i32,
        actual: usize,
        data: Vec<u8>,
        data_start: usize,
        iso: Vec<IsoPacket>,
    ) -> Self {
        let mut header = [0; 48];
        put32(&mut header, RET_SUBMIT);
        put32(&mut header[4..], req.seq);
        put32(&mut header[20..], status as u32);
        put32(&mut header[24..], actual as u32);
        put32(&mut header[28..], req.start_frame);
        put32(&mut header[32..], req.packet_count);
        put32(
            &mut header[36..],
            iso.iter().filter(|p| p.status != 0).count() as u32,
        );
        Self {
            header,
            data,
            data_start,
            data_len: if req.direction == 1 { actual } else { 0 },
            iso,
            memory: None,
        }
    }
    pub fn unlink(seq: u32, status: i32) -> Self {
        let mut header = [0; 48];
        put32(&mut header, RET_UNLINK);
        put32(&mut header[4..], seq);
        put32(&mut header[20..], status as u32);
        Self {
            header,
            data: Vec::new(),
            data_start: 0,
            data_len: 0,
            iso: Vec::new(),
            memory: None,
        }
    }
    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        let mut iso = Vec::with_capacity(self.iso.len() * 16);
        for p in &self.iso {
            for v in [p.offset, p.length, p.actual, p.status as u32] {
                iso.extend_from_slice(&v.to_be_bytes());
            }
        }
        let data = self
            .data
            .get(self.data_start..self.data_start + self.data_len)
            .ok_or_else(|| invalid("completion payload bounds"))?;
        let mut slices = [
            IoSlice::new(&self.header),
            IoSlice::new(data),
            IoSlice::new(&iso),
        ];
        write_vectored_all(w, &mut slices)
    }
}
