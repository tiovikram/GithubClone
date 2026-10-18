//! A minimal ZIP reader (stored and deflate entries), enough for osv.dev's `all.zip` exports.
use std::io::Read;

fn u16le(b: &[u8], o: usize) -> anyhow::Result<u16> {
    Ok(u16::from_le_bytes(b.get(o..o + 2).ok_or_else(|| anyhow::anyhow!("truncated zip"))?.try_into()?))
}
fn u32le(b: &[u8], o: usize) -> anyhow::Result<u32> {
    Ok(u32::from_le_bytes(b.get(o..o + 4).ok_or_else(|| anyhow::anyhow!("truncated zip"))?.try_into()?))
}

/// Maximum uncompressed size of one entry (OSV records are small).
const MAX_ENTRY: u64 = 64 * 1024 * 1024;

/// All file entries as (name, data).
pub fn read_all(b: &[u8]) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    // End of central directory: last occurrence of the signature within the final 64 KiB.
    let start = b.len().saturating_sub(65_557);
    let eocd = (start..b.len().saturating_sub(21))
        .rev()
        .find(|&i| b[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(|| anyhow::anyhow!("not a zip file"))?;
    let entries = u16le(b, eocd + 10)? as usize;
    let mut off = u32le(b, eocd + 16)? as usize;
    let mut out = Vec::with_capacity(entries);
    for _ in 0..entries {
        if u32le(b, off)? != 0x0201_4b50 {
            anyhow::bail!("bad central directory");
        }
        let method = u16le(b, off + 10)?;
        let csize = u32le(b, off + 20)? as usize;
        let usize_ = u32le(b, off + 24)? as u64;
        let nlen = u16le(b, off + 28)? as usize;
        let xlen = u16le(b, off + 30)? as usize;
        let clen = u16le(b, off + 32)? as usize;
        let local = u32le(b, off + 42)? as usize;
        let name = String::from_utf8_lossy(b.get(off + 46..off + 46 + nlen).ok_or_else(|| anyhow::anyhow!("truncated zip"))?).into_owned();
        off += 46 + nlen + xlen + clen;
        if name.ends_with('/') {
            continue;
        }
        if u32le(b, local)? != 0x0403_4b50 {
            anyhow::bail!("bad local header");
        }
        let data_start = local + 30 + u16le(b, local + 26)? as usize + u16le(b, local + 28)? as usize;
        let raw = b.get(data_start..data_start + csize).ok_or_else(|| anyhow::anyhow!("truncated zip entry"))?;
        if usize_ > MAX_ENTRY {
            continue;
        }
        let data = match method {
            0 => raw.to_vec(),
            8 => {
                let mut d = Vec::with_capacity(usize_ as usize);
                flate2::read::DeflateDecoder::new(raw).take(MAX_ENTRY).read_to_end(&mut d)?;
                d
            }
            _ => continue,
        };
        out.push((name, data));
    }
    Ok(out)
}
