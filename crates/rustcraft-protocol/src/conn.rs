//! Packet framing: a VarInt length, optional zlib compression above a
//! threshold, and optional AES-128/CFB8 encryption of the whole stream.
//! Reading and writing are separate halves, so each can run on its own
//! thread.

use std::io::{BufReader, Read, Write};

use aes::cipher::inout::InOutBuf;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

use crate::codec::{Buf, Put};
use crate::{Error, Result};

type Encryptor = cfb8::Encryptor<aes::Aes128>;
type Decryptor = cfb8::Decryptor<aes::Aes128>;

/// A frame's length is a VarInt of at most 3 bytes.
pub const MAX_FRAME: usize = (1 << 21) - 1;
/// Largest packet after decompression; vanilla refuses bigger ones too.
pub const MAX_PACKET: usize = 1 << 21;

pub struct PacketReader<R> {
    inner: BufReader<R>,
    cipher: Option<Decryptor>,
    threshold: Option<usize>,
}

impl<R: Read> PacketReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner: BufReader::new(inner), cipher: None, threshold: None }
    }

    /// Decrypts everything read from now on. Bytes already buffered are
    /// still undecrypted, so this is correct at any packet boundary.
    pub fn enable_encryption(&mut self, secret: &[u8; 16]) {
        self.cipher = Some(Decryptor::new(secret.into(), secret.into()));
    }

    /// `None` (or a negative threshold from the server) turns compression off.
    pub fn set_compression(&mut self, threshold: Option<usize>) {
        self.threshold = threshold;
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        self.inner.read_exact(buf)?;
        if let Some(c) = &mut self.cipher {
            let (blocks, _) = InOutBuf::from(buf).into_chunks();
            c.decrypt_blocks_inout_mut(blocks);
        }
        Ok(())
    }

    /// The next packet as (id, body).
    pub fn read(&mut self) -> Result<(i32, Vec<u8>)> {
        let mut len = 0usize;
        for i in 0..3 {
            let mut b = [0];
            self.read_exact(&mut b)?;
            len |= ((b[0] & 0x7F) as usize) << (7 * i);
            if b[0] & 0x80 == 0 {
                break;
            }
            if i == 2 {
                return Err(Error::Malformed("frame length longer than 3 bytes"));
            }
        }
        let mut frame = vec![0; len];
        self.read_exact(&mut frame)?;
        let packet = match self.threshold {
            None => frame,
            Some(_) => {
                let mut b = Buf(&frame);
                let size = b.length(MAX_PACKET)?;
                if size == 0 {
                    b.rest().to_vec()
                } else {
                    let mut out = Vec::with_capacity(size);
                    ZlibDecoder::new(b.rest()).take(size as u64 + 1).read_to_end(&mut out)?;
                    if out.len() != size {
                        return Err(Error::Malformed("compressed packet has the wrong size"));
                    }
                    out
                }
            }
        };
        let mut b = Buf(&packet);
        let id = b.varint()?;
        Ok((id, b.rest().to_vec()))
    }
}

pub struct PacketWriter<W> {
    inner: W,
    cipher: Option<Encryptor>,
    threshold: Option<usize>,
}

impl<W: Write> PacketWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner, cipher: None, threshold: None }
    }

    pub fn enable_encryption(&mut self, secret: &[u8; 16]) {
        self.cipher = Some(Encryptor::new(secret.into(), secret.into()));
    }

    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    pub fn set_compression(&mut self, threshold: Option<usize>) {
        self.threshold = threshold;
    }

    pub fn write(&mut self, id: i32, body: &[u8]) -> Result<()> {
        let mut packet = Vec::with_capacity(body.len() + 5);
        packet.varint(id).be(body);
        let content = match self.threshold {
            None => packet,
            Some(t) if packet.len() < t => {
                let mut v = vec![0];
                v.extend_from_slice(&packet);
                v
            }
            Some(_) => {
                let mut v = Vec::new();
                v.varint(packet.len() as i32);
                let mut z = ZlibEncoder::new(v, Compression::default());
                z.write_all(&packet)?;
                z.finish()?
            }
        };
        if content.len() > MAX_FRAME {
            return Err(Error::Malformed("packet too large to send"));
        }
        let mut frame = Vec::with_capacity(content.len() + 3);
        frame.varint(content.len() as i32).be(&content);
        if let Some(c) = &mut self.cipher {
            let (blocks, _) = InOutBuf::from(frame.as_mut_slice()).into_chunks();
            c.encrypt_blocks_inout_mut(blocks);
        }
        self.inner.write_all(&frame)?;
        Ok(self.inner.flush()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfb8_matches_nist_vector() {
        // NIST SP 800-38A, F.3.7 CFB8-AES128.Encrypt.
        let key: [u8; 16] = hex::decode("2b7e151628aed2a6abf7158809cf4f3c").unwrap().try_into().unwrap();
        let iv: [u8; 16] = hex::decode("000102030405060708090a0b0c0d0e0f").unwrap().try_into().unwrap();
        let mut data = hex::decode("6bc1bee22e409f96e93d7e117393172aae2d").unwrap();
        let mut c = Encryptor::new(&key.into(), &iv.into());
        // In two pieces, as a stream would arrive.
        let (a, b) = data.split_at_mut(7);
        for part in [a, b] {
            let (blocks, _) = InOutBuf::from(part).into_chunks();
            c.encrypt_blocks_inout_mut(blocks);
        }
        assert_eq!(hex::encode(&data), "3b79424c9c0dd436bace9e0ed4586a4f32b9");
    }

    #[test]
    fn frames_round_trip_with_compression_and_encryption() {
        let secret = [7u8; 16];
        let mut wire = Vec::new();
        {
            let mut w = PacketWriter::new(&mut wire);
            w.write(0x01, b"plain").unwrap();
            w.set_compression(Some(64));
            w.write(0x02, b"small").unwrap();
            w.write(0x03, &[9u8; 5000]).unwrap();
            w.enable_encryption(&secret);
            w.write(0x04, &[1u8; 300]).unwrap();
        }
        // Compression worked: 5000 identical bytes became a short frame.
        assert!(wire.len() < 1000, "{}", wire.len());
        let mut r = PacketReader::new(wire.as_slice());
        assert_eq!(r.read().unwrap(), (0x01, b"plain".to_vec()));
        r.set_compression(Some(64));
        assert_eq!(r.read().unwrap(), (0x02, b"small".to_vec()));
        assert_eq!(r.read().unwrap(), (0x03, vec![9u8; 5000]));
        r.enable_encryption(&secret);
        assert_eq!(r.read().unwrap(), (0x04, vec![1u8; 300]));
        assert!(matches!(r.read(), Err(Error::Io(_))));
    }

    #[test]
    fn oversized_and_lying_frames_are_rejected() {
        let mut r = PacketReader::new(&[0xff, 0xff, 0xff, 0x01][..]);
        assert!(matches!(r.read(), Err(Error::Malformed(_))));
        // A compressed packet that claims more bytes than it inflates to.
        let mut packet = Vec::new();
        packet.varint(1);
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        z.write_all(&packet).unwrap();
        let mut content = Vec::new();
        content.varint(50).be(&z.finish().unwrap());
        let mut frame = Vec::new();
        frame.varint(content.len() as i32).be(&content);
        let mut r = PacketReader::new(frame.as_slice());
        r.set_compression(Some(0));
        assert!(matches!(r.read(), Err(Error::Malformed(_))));
    }
}
