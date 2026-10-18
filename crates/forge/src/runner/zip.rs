//! Minimal ZIP archives (deflate or stored, no ZIP64): run log downloads and artifacts.
use std::io::{Read, Write};

pub struct ZipWriter {
    buf: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

fn dos_time(t: chrono::DateTime<chrono::Utc>) -> (u16, u16) {
    use chrono::{Datelike, Timelike};
    let time = ((t.hour() as u16) << 11) | ((t.minute() as u16) << 5) | ((t.second() as u16) / 2);
    let date = (((t.year().max(1980) - 1980) as u16) << 9) | ((t.month() as u16) << 5) | (t.day() as u16);
    (time, date)
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZipWriter {
    pub fn new() -> Self {
        ZipWriter { buf: Vec::new(), central: Vec::new(), count: 0 }
    }

    /// Adds a file (deflated). `name` uses `/` separators.
    pub fn add(&mut self, name: &str, data: &[u8]) {
        let crc = crc32fast::hash(data);
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = enc.write_all(data);
        let deflated = enc.finish().unwrap_or_default();
        let (method, body): (u16, &[u8]) = if deflated.len() < data.len() { (8, &deflated) } else { (0, data) };
        let (time, date) = dos_time(chrono::Utc::now());
        let offset = self.buf.len() as u32;
        let name_b = name.as_bytes();
        let flags: u16 = 0x0800; // UTF-8 names
        // local header
        self.buf.extend_from_slice(&0x04034b50u32.to_le_bytes());
        self.buf.extend_from_slice(&20u16.to_le_bytes());
        self.buf.extend_from_slice(&flags.to_le_bytes());
        self.buf.extend_from_slice(&method.to_le_bytes());
        self.buf.extend_from_slice(&time.to_le_bytes());
        self.buf.extend_from_slice(&date.to_le_bytes());
        self.buf.extend_from_slice(&crc.to_le_bytes());
        self.buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
        self.buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.buf.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf.extend_from_slice(name_b);
        self.buf.extend_from_slice(body);
        // central directory entry
        let c = &mut self.central;
        c.extend_from_slice(&0x02014b50u32.to_le_bytes());
        c.extend_from_slice(&(0x0300u16 | 20).to_le_bytes()); // made by unix
        c.extend_from_slice(&20u16.to_le_bytes());
        c.extend_from_slice(&flags.to_le_bytes());
        c.extend_from_slice(&method.to_le_bytes());
        c.extend_from_slice(&time.to_le_bytes());
        c.extend_from_slice(&date.to_le_bytes());
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&(body.len() as u32).to_le_bytes());
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes()); // extra
        c.extend_from_slice(&0u16.to_le_bytes()); // comment
        c.extend_from_slice(&0u16.to_le_bytes()); // disk
        c.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        c.extend_from_slice(&((0o100644u32) << 16).to_le_bytes()); // external attrs
        c.extend_from_slice(&offset.to_le_bytes());
        c.extend_from_slice(name_b);
        self.count += 1;
    }

    pub fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.buf.len() as u32;
        let cd_size = self.central.len() as u32;
        self.buf.extend_from_slice(&self.central);
        self.buf.extend_from_slice(&0x06054b50u32.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf.extend_from_slice(&self.count.to_le_bytes());
        self.buf.extend_from_slice(&self.count.to_le_bytes());
        self.buf.extend_from_slice(&cd_size.to_le_bytes());
        self.buf.extend_from_slice(&cd_offset.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf
    }
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Reads all files of an archive: (name, contents). Directories are skipped.
pub fn read_all(b: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    // end of central directory
    let eocd = (0..b.len().saturating_sub(21)).rev().find(|&i| u32_at(b, i) == Some(0x06054b50)).ok_or("not a zip archive")?;
    let count = u16_at(b, eocd + 10).ok_or("bad zip")? as usize;
    let mut p = u32_at(b, eocd + 16).ok_or("bad zip")? as usize;
    let mut out = Vec::new();
    for _ in 0..count {
        if u32_at(b, p) != Some(0x02014b50) {
            return Err("bad central directory".into());
        }
        let method = u16_at(b, p + 10).ok_or("bad zip")?;
        let csize = u32_at(b, p + 20).ok_or("bad zip")? as usize;
        let usize_ = u32_at(b, p + 24).ok_or("bad zip")? as usize;
        let nlen = u16_at(b, p + 28).ok_or("bad zip")? as usize;
        let elen = u16_at(b, p + 30).ok_or("bad zip")? as usize;
        let clen = u16_at(b, p + 32).ok_or("bad zip")? as usize;
        let lho = u32_at(b, p + 42).ok_or("bad zip")? as usize;
        let name = String::from_utf8_lossy(b.get(p + 46..p + 46 + nlen).ok_or("bad zip")?).to_string();
        p += 46 + nlen + elen + clen;
        if name.ends_with('/') {
            continue;
        }
        let lnlen = u16_at(b, lho + 26).ok_or("bad zip")? as usize;
        let lelen = u16_at(b, lho + 28).ok_or("bad zip")? as usize;
        let start = lho + 30 + lnlen + lelen;
        let data = b.get(start..start + csize).ok_or("truncated zip")?;
        let content = match method {
            0 => data.to_vec(),
            8 => {
                let mut d = flate2::read::DeflateDecoder::new(data);
                let mut v = Vec::with_capacity(usize_);
                d.read_to_end(&mut v).map_err(|e| e.to_string())?;
                v
            }
            m => return Err(format!("unsupported zip compression method {m}")),
        };
        if name.contains("..") || name.starts_with('/') {
            return Err(format!("unsafe path in archive: {name}"));
        }
        out.push((name, content));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        let mut z = super::ZipWriter::new();
        z.add("a.txt", b"hello hello hello hello hello");
        z.add("dir/b.bin", &[0, 1, 2]);
        let bytes = z.finish();
        let files = super::read_all(&bytes).unwrap();
        assert_eq!(files[0], ("a.txt".to_string(), b"hello hello hello hello hello".to_vec()));
        assert_eq!(files[1].0, "dir/b.bin");
    }
}
