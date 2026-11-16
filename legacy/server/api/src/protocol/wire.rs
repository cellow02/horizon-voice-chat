//! Minecraft's network encoding: big-endian numbers, VarInt lengths, UUIDs as 16 bytes.

/// Why a body could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    /// The body ended before the value did.
    UnexpectedEnd,
    /// A VarInt ran past five bytes.
    VarIntTooLong,
    /// A length was negative or above its limit.
    BadLength,
    /// A string was not UTF-8.
    Utf8,
    /// A field held a value the protocol does not define.
    InvalidValue,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Reads values from a borrowed body. Never panics on any input.
pub struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
    pub fn remaining(&self) -> usize {
        self.data.len()
    }
    pub fn take(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        if self.data.len() < len {
            return Err(WireError::UnexpectedEnd);
        }
        let (head, tail) = self.data.split_at(len);
        self.data = tail;
        Ok(head)
    }
    /// Everything that is left.
    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.data)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }
    pub fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    pub fn bool(&mut self) -> Result<bool, WireError> {
        Ok(self.u8()? != 0)
    }
    pub fn i32(&mut self) -> Result<i32, WireError> {
        Ok(i32::from_be_bytes(self.array()?))
    }
    pub fn i64(&mut self) -> Result<i64, WireError> {
        Ok(i64::from_be_bytes(self.array()?))
    }
    pub fn f32(&mut self) -> Result<f32, WireError> {
        Ok(f32::from_be_bytes(self.array()?))
    }
    pub fn f64(&mut self) -> Result<f64, WireError> {
        Ok(f64::from_be_bytes(self.array()?))
    }
    /// A UUID: most significant half first.
    pub fn uuid(&mut self) -> Result<u128, WireError> {
        Ok(u128::from_be_bytes(self.array()?))
    }
    pub fn bytes16(&mut self) -> Result<[u8; 16], WireError> {
        self.array()
    }
    pub fn var_int(&mut self) -> Result<i32, WireError> {
        let mut value: u32 = 0;
        for shift in 0..5 {
            let byte = self.u8()?;
            value |= u32::from(byte & 0x7f) << (7 * shift);
            if byte & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(WireError::VarIntTooLong)
    }
    /// A VarInt length, then that many bytes; refused above `max`.
    pub fn byte_array(&mut self, max: usize) -> Result<&'a [u8], WireError> {
        let len = usize::try_from(self.var_int()?).map_err(|_| WireError::BadLength)?;
        if len > max {
            return Err(WireError::BadLength);
        }
        self.take(len)
    }
    /// A VarInt byte length, then UTF-8; refused above `max_chars` characters.
    pub fn string(&mut self, max_chars: usize) -> Result<String, WireError> {
        let bytes = self.byte_array(max_chars.saturating_mul(3))?;
        let text = std::str::from_utf8(bytes).map_err(|_| WireError::Utf8)?;
        if text.chars().count() > max_chars {
            return Err(WireError::BadLength);
        }
        Ok(text.to_owned())
    }
}

/// Writes values into a growing body.
#[derive(Default)]
pub struct Writer {
    pub data: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn finish(self) -> Vec<u8> {
        self.data
    }
    pub fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.data.extend_from_slice(bytes);
        self
    }
    pub fn u8(&mut self, value: u8) -> &mut Self {
        self.data.push(value);
        self
    }
    pub fn bool(&mut self, value: bool) -> &mut Self {
        self.u8(u8::from(value))
    }
    pub fn i32(&mut self, value: i32) -> &mut Self {
        self.raw(&value.to_be_bytes())
    }
    pub fn i64(&mut self, value: i64) -> &mut Self {
        self.raw(&value.to_be_bytes())
    }
    pub fn f32(&mut self, value: f32) -> &mut Self {
        self.raw(&value.to_be_bytes())
    }
    pub fn f64(&mut self, value: f64) -> &mut Self {
        self.raw(&value.to_be_bytes())
    }
    pub fn uuid(&mut self, value: u128) -> &mut Self {
        self.raw(&value.to_be_bytes())
    }
    pub fn var_int(&mut self, value: i32) -> &mut Self {
        let mut value = value as u32;
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                return self.u8(byte);
            }
            self.u8(byte | 0x80);
        }
    }
    pub fn byte_array(&mut self, bytes: &[u8]) -> &mut Self {
        self.var_int(bytes.len() as i32);
        self.raw(bytes)
    }
    pub fn string(&mut self, text: &str) -> &mut Self {
        self.byte_array(text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var_ints_round_trip_and_match_known_bytes() {
        for (value, bytes) in [
            (0, vec![0u8]),
            (127, vec![0x7f]),
            (128, vec![0x80, 0x01]),
            (25565, vec![0xdd, 0xc7, 0x01]),
            (-1, vec![0xff, 0xff, 0xff, 0xff, 0x0f]),
        ] {
            let mut writer = Writer::new();
            writer.var_int(value);
            assert_eq!(writer.data, bytes);
            assert_eq!(Reader::new(&bytes).var_int(), Ok(value));
        }
        assert_eq!(
            Reader::new(&[0xff; 6]).var_int(),
            Err(WireError::VarIntTooLong)
        );
    }

    #[test]
    fn short_or_oversized_input_is_refused_not_panicked() {
        assert_eq!(Reader::new(&[1, 2]).i32(), Err(WireError::UnexpectedEnd));
        assert_eq!(
            Reader::new(&[5, 1, 2]).byte_array(16),
            Err(WireError::UnexpectedEnd)
        );
        assert_eq!(
            Reader::new(&[5, 1, 2, 3, 4, 5]).byte_array(4),
            Err(WireError::BadLength)
        );
        assert_eq!(Reader::new(&[2, 0xff, 0xfe]).string(8), Err(WireError::Utf8));
    }

    #[test]
    fn uuids_are_sixteen_big_endian_bytes() {
        let uuid = 0x0123_4567_89ab_cdef_0011_2233_4455_6677_u128;
        let mut writer = Writer::new();
        writer.uuid(uuid);
        assert_eq!(writer.data[0], 0x01);
        assert_eq!(writer.data[15], 0x77);
        assert_eq!(Reader::new(&writer.data).uuid(), Ok(uuid));
    }
}
