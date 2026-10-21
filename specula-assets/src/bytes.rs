//! A cursor over little-endian bytes, which the browser's decoders (stream,
//! terrain, icons, selection atlas, unit files) all read with.

/// `.0` is what is left to read, so callers can test for trailing bytes.
pub struct ByteReader<'a>(pub &'a [u8]);

impl<'a> ByteReader<'a> {
    pub fn take(&mut self, length: usize) -> Result<&'a [u8], &'static str> {
        if self.0.len() < length {
            return Err("data truncated");
        }
        let (bytes, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(bytes)
    }

    pub fn u8(&mut self) -> Result<u8, &'static str> {
        let (&value, rest) = self.0.split_first().ok_or("data truncated")?;
        self.0 = rest;
        Ok(value)
    }

    pub fn u16(&mut self) -> Result<u16, &'static str> {
        let (bytes, rest) = self.0.split_first_chunk::<2>().ok_or("data truncated")?;
        self.0 = rest;
        Ok(u16::from_le_bytes(*bytes))
    }

    pub fn u32(&mut self) -> Result<u32, &'static str> {
        let (bytes, rest) = self.0.split_first_chunk::<4>().ok_or("data truncated")?;
        self.0 = rest;
        Ok(u32::from_le_bytes(*bytes))
    }

    pub fn u64(&mut self) -> Result<u64, &'static str> {
        let (bytes, rest) = self.0.split_first_chunk::<8>().ok_or("data truncated")?;
        self.0 = rest;
        Ok(u64::from_le_bytes(*bytes))
    }

    pub fn f32(&mut self) -> Result<f32, &'static str> {
        self.u32().map(f32::from_bits)
    }

    pub fn varint(&mut self) -> Result<u64, &'static str> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("varint too long")
    }

    pub fn varint_u32(&mut self) -> Result<u32, &'static str> {
        u32::try_from(self.varint()?).map_err(|_| "varint out of range")
    }

    pub fn zigzag_i64(&mut self) -> Result<i64, &'static str> {
        let raw = self.varint()?;
        Ok((raw >> 1) as i64 ^ -((raw & 1) as i64))
    }

    pub fn zigzag(&mut self) -> Result<i32, &'static str> {
        i32::try_from(self.zigzag_i64()?).map_err(|_| "zigzag out of range")
    }

    /// varint length, UTF-8.
    pub fn text(&mut self) -> Result<String, &'static str> {
        let length = self.varint()? as usize;
        let text = std::str::from_utf8(self.take(length)?).map_err(|_| "string not UTF-8")?;
        Ok(text.to_string())
    }

    /// u16 length and UTF-8.
    pub fn string16(&mut self) -> Result<String, &'static str> {
        let length = usize::from(self.u16()?);
        let text = std::str::from_utf8(self.take(length)?).map_err(|_| "string not UTF-8")?;
        Ok(text.to_string())
    }
}
