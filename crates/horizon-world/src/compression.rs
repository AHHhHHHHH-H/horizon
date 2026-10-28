//! Region codecs, including lz4-java's block framing (not the LZ4 frame format).
use flate2::{Compression as Level, Decompress, FlushDecompress, Status, write::ZlibEncoder};
use std::{
    cell::RefCell,
    io::{self, Read, Write},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Compression {
    Gzip = 1,
    Zlib = 2,
    None = 3,
    Lz4 = 4,
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// One zlib stream and its scratch per thread: a saved chunk allocates a
/// 256 KiB deflate state no more, and `reset` clears it between chunks.
struct Zlib {
    compressor: Option<ZlibEncoder<Vec<u8>>>,
    decompressor: Option<Decompress>,
    block: [u8; 8192],
}
thread_local! {
    static ZLIB: RefCell<Zlib> = const {
        RefCell::new(Zlib {
            compressor: None,
            decompressor: None,
            block: [0; 8192],
        })
    };
}

impl TryFrom<u8> for Compression {
    type Error = io::Error;
    fn try_from(value: u8) -> io::Result<Self> {
        match value {
            1 => Ok(Self::Gzip),
            2 => Ok(Self::Zlib),
            3 => Ok(Self::None),
            4 => Ok(Self::Lz4),
            _ => Err(invalid(format!(
                "unsupported region compression ID {value}"
            ))),
        }
    }
}

impl Compression {
    pub fn decode(self, bytes: &[u8], max_output: usize) -> io::Result<Vec<u8>> {
        match self {
            Self::None => {
                if bytes.len() > max_output {
                    return Err(invalid("decompression limit exceeded"));
                }
                Ok(bytes.to_vec())
            }
            Self::Gzip => {
                let decoder = flate2::read::MultiGzDecoder::new(bytes);
                let mut output = Vec::new();
                decoder
                    .take((max_output as u64).saturating_add(1))
                    .read_to_end(&mut output)?;
                if output.len() > max_output {
                    return Err(invalid("decompression limit exceeded"));
                }
                Ok(output)
            }
            Self::Zlib => ZLIB.with(|zlib| {
                let mut state = zlib.borrow_mut();
                let Zlib {
                    decompressor,
                    block,
                    ..
                } = &mut *state;
                let decoder = decompressor.get_or_insert_with(|| Decompress::new(true));
                decoder.reset(true);
                let mut output = Vec::new();
                let result = loop {
                    let before_in = decoder.total_in();
                    let before_out = decoder.total_out();
                    let status = match decoder.decompress(
                        &bytes[before_in as usize..],
                        block,
                        FlushDecompress::None,
                    ) {
                        Ok(status) => status,
                        Err(e) => break Err(invalid(e.to_string())),
                    };
                    let produced = (decoder.total_out() - before_out) as usize;
                    if produced > max_output.saturating_sub(output.len()) {
                        break Err(invalid("decompression limit exceeded"));
                    }
                    output.extend_from_slice(&block[..produced]);
                    if status == Status::StreamEnd {
                        break Ok(output);
                    }
                    if decoder.total_in() == before_in && produced == 0 {
                        break Err(invalid("truncated zlib stream"));
                    }
                };
                decoder.reset(true);
                result
            }),
            Self::Lz4 => decode_lz4(bytes, max_output),
        }
    }

    pub fn encode(self, bytes: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Self::None => Ok(bytes.to_vec()),
            Self::Gzip => {
                let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Level::default());
                encoder.write_all(bytes)?;
                encoder.finish()
            }
            Self::Zlib => ZLIB.with(|zlib| {
                let mut state = zlib.borrow_mut();
                let encoder = state
                    .compressor
                    .get_or_insert_with(|| ZlibEncoder::new(Vec::new(), Level::default()));
                encoder.get_mut().clear();
                let result = encoder.write_all(bytes).and_then(|()| encoder.try_finish());
                let encoded = std::mem::take(encoder.get_mut());
                let _ = encoder.reset(Vec::new());
                result.map(|()| encoded)
            }),
            Self::Lz4 => Ok(encode_lz4(bytes)),
        }
    }
}

const MAGIC: &[u8; 8] = b"LZ4Block";
fn checksum(bytes: &[u8]) -> u32 {
    // The Checksum adapter used by the pinned Java library masks to 28 bits.
    xxhash_rust::xxh32::xxh32(bytes, 0x9747_b28c) & 0x0fff_ffff
}

fn decode_lz4(mut bytes: &[u8], max_output: usize) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        let header = bytes
            .get(..21)
            .ok_or_else(|| invalid("truncated LZ4 header"))?;
        if &header[..8] != MAGIC {
            return Err(invalid("invalid LZ4 block magic"));
        }
        let method = header[8] & 0xf0;
        if !matches!(method, 0x10 | 0x20) {
            return Err(invalid("invalid LZ4 block method"));
        }
        let compressed = u32::from_le_bytes(header[9..13].try_into().unwrap()) as usize;
        let raw = u32::from_le_bytes(header[13..17].try_into().unwrap()) as usize;
        let expected = u32::from_le_bytes(header[17..21].try_into().unwrap());
        if raw > (1 << (10 + (header[8] & 15)))
            || (raw == 0) != (compressed == 0)
            || (method == 0x10 && raw != compressed)
        {
            return Err(invalid("invalid LZ4 block lengths"));
        }
        bytes = &bytes[21..];
        if raw == 0 {
            if expected != 0 {
                return Err(invalid("invalid LZ4 terminator checksum"));
            }
            return Ok(output);
        }
        if raw > max_output.saturating_sub(output.len()) {
            return Err(invalid("decompression limit exceeded"));
        }
        let block = bytes
            .get(..compressed)
            .ok_or_else(|| invalid("truncated LZ4 payload"))?;
        let start = output.len();
        if method == 0x10 {
            output.extend_from_slice(block);
        } else {
            output.resize(start + raw, 0);
            let actual = lz4_flex::block::decompress_into(block, &mut output[start..])
                .map_err(|e| invalid(e.to_string()))?;
            if actual != raw {
                return Err(invalid("LZ4 decoded length mismatch"));
            }
        }
        if checksum(&output[start..]) != expected {
            return Err(invalid("LZ4 checksum mismatch"));
        }
        bytes = &bytes[compressed..];
    }
}

fn encode_lz4(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    for block in bytes.chunks(65536) {
        let compressed = lz4_flex::block::compress(block);
        let (token, payload) = if compressed.len() < block.len() {
            (0x26, compressed.as_slice())
        } else {
            (0x16, block)
        };
        output.extend_from_slice(MAGIC);
        output.push(token);
        output.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        output.extend_from_slice(&(block.len() as u32).to_le_bytes());
        output.extend_from_slice(&checksum(block).to_le_bytes());
        output.extend_from_slice(payload);
    }
    output.extend_from_slice(MAGIC);
    output.push(0x16);
    output.extend_from_slice(&[0; 12]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_codecs_roundtrip_and_enforce_limits() {
        for codec in [
            Compression::Gzip,
            Compression::Zlib,
            Compression::None,
            Compression::Lz4,
        ] {
            for bytes in [
                vec![],
                vec![0; 131073],
                (0..262144).map(|i| (i * 37) as u8).collect(),
            ] {
                let encoded = codec.encode(&bytes).unwrap();
                assert_eq!(codec.decode(&encoded, bytes.len()).unwrap(), bytes);
                if !bytes.is_empty() {
                    assert!(codec.decode(&encoded, bytes.len() - 1).is_err());
                }
                if codec != Compression::None {
                    assert!(
                        codec
                            .decode(&encoded[..encoded.len() - 1], bytes.len())
                            .is_err()
                    );
                }
            }
        }
    }
    #[test]
    fn zlib_streams_are_reused_and_reset_after_errors() {
        let data: Vec<u8> = (0..200_000).map(|i| (i * 31) as u8).collect();
        // Reused thread-local streams stay independent, including after an
        // error truncated or over-limit decode.
        for _ in 0..64 {
            let encoded = Compression::Zlib.encode(&data).unwrap();
            assert_eq!(
                Compression::Zlib.decode(&encoded, data.len()).unwrap(),
                data
            );
        }
        let encoded = Compression::Zlib.encode(b"reset me").unwrap();
        assert!(
            Compression::Zlib
                .decode(&encoded[..encoded.len() - 1], 100)
                .is_err()
        );
        assert!(Compression::Zlib.decode(&encoded, 3).is_err());
        assert_eq!(
            Compression::Zlib.decode(&encoded, 100).unwrap(),
            b"reset me"
        );
        assert!(Compression::Zlib.encode(&data).is_ok());
    }
    #[test]
    fn checksums_reject_corruption() {
        for codec in [Compression::Gzip, Compression::Zlib, Compression::Lz4] {
            let mut encoded = codec.encode(b"Horizon world data").unwrap();
            let i = if codec == Compression::Lz4 {
                17
            } else {
                encoded.len() - 5
            };
            encoded[i] ^= 1;
            assert!(codec.decode(&encoded, 1000).is_err());
        }
    }
}
