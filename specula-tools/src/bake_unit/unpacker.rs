//! The engine's packed file layout, `CFileUnpacker` in ps/FileIo.cpp: a
//! 4-byte magic, u32 version, u32 payload size, then the payload.

use crate::math::Quat;
use crate::math::Vec3;

pub struct Unpacker<'a> {
    data: &'a [u8],
    pub version: u32,
}

impl<'a> Unpacker<'a> {
    pub fn new(data: &'a [u8], magic: &[u8; 4]) -> Result<Self, String> {
        if data.len() < 12 {
            return Err("shorter than its header".into());
        }
        if &data[..4] != magic {
            return Err(format!("not a {} file", String::from_utf8_lossy(magic)));
        }
        let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let payload = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
        if data.len() != 12 + payload {
            return Err("payload size does not match the file".into());
        }
        Ok(Unpacker {
            data: &data[12..],
            version,
        })
    }

    pub fn bytes(&mut self, length: usize) -> Result<&'a [u8], String> {
        if self.data.len() < length {
            return Err("truncated".into());
        }
        let (bytes, rest) = self.data.split_at(length);
        self.data = rest;
        Ok(bytes)
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    /// `UnpackSize`: a u32 count.
    pub fn size(&mut self) -> Result<usize, String> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()) as usize)
    }

    pub fn string(&mut self) -> Result<String, String> {
        let length = self.size()?;
        Ok(String::from_utf8_lossy(self.bytes(length)?).into_owned())
    }

    pub fn vec3(&mut self) -> Result<Vec3, String> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }

    /// CQuaternion is stored as its vector part, then w.
    pub fn quat(&mut self) -> Result<Quat, String> {
        Ok(Quat {
            x: self.f32()?,
            y: self.f32()?,
            z: self.f32()?,
            w: self.f32()?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}
