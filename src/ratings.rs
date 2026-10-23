//! IMDb's ratings dataset (`title.ratings.tsv.gz`, refreshed daily):
//! `tconst \t averageRating \t numVotes`, one row per rated title.
//! Pure: bytes in, a lookup table out.

use std::io::{BufRead, BufReader, Read};

use flate2::read::GzDecoder;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rating {
    pub rating: f32,
    pub votes: u32,
}

/// Every rated title, sorted by numeric id: ~1.7M rows, 12 bytes each.
#[derive(Debug, Default)]
pub struct Table {
    rows: Vec<Row>,
}

#[derive(Debug, Clone, Copy)]
struct Row {
    id: u32,
    votes: u32,
    /// Rating × 10, so 7.8 is 78.
    rating: u16,
}

impl Table {
    /// Parses the dataset, gzipped or not. Malformed rows are skipped; a
    /// body with no rows at all is an error (a broken download, not data).
    pub fn parse(body: &[u8]) -> Result<Self, String> {
        let reader: Box<dyn Read + '_> =
            if body.starts_with(&[0x1f, 0x8b]) { Box::new(GzDecoder::new(body)) } else { Box::new(body) };
        let mut reader = BufReader::with_capacity(1 << 16, reader);

        // 1.72M rows in October 2026. Room to grow: outgrowing the capacity
        // doubles the buffer, and wasm memory never shrinks back.
        let mut rows = Vec::with_capacity(2_000_000);
        let mut line = Vec::with_capacity(64);
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => return Err(format!("dataset unreadable: {e}")),
            }
            if let Some(row) = parse_row(&line) {
                rows.push(row);
            }
        }
        if rows.is_empty() {
            return Err("dataset has no ratings".to_owned());
        }
        rows.sort_unstable_by_key(|r| r.id);
        Ok(Self { rows })
    }

    /// The rating for an IMDb id like `tt0133093`.
    pub fn get(&self, imdb_id: &str) -> Option<Rating> {
        let id = parse_id(imdb_id.as_bytes())?;
        let i = self.rows.binary_search_by_key(&id, |r| r.id).ok()?;
        let row = self.rows[i];
        Some(Rating { rating: f32::from(row.rating) / 10.0, votes: row.votes })
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn parse_row(line: &[u8]) -> Option<Row> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let mut fields = line.split(|&b| b == b'\t');
    let id = parse_id(fields.next()?)?;
    let rating = parse_tenths(fields.next()?)?;
    let votes = parse_u32(fields.next()?)?;
    Some(Row { id, votes, rating })
}

/// `tt0133093` → 133093. Anything else (the header row included) → `None`.
pub fn parse_id(text: &[u8]) -> Option<u32> {
    parse_u32(text.strip_prefix(b"tt")?)
}

/// `7.8` → 78, `10.0` → 100, `7` → 70.
fn parse_tenths(text: &[u8]) -> Option<u16> {
    let (whole, frac) = match text.iter().position(|&b| b == b'.') {
        Some(dot) => (&text[..dot], &text[dot + 1..]),
        None => (text, &b""[..]),
    };
    let tenths = match frac.first() {
        Some(d) if d.is_ascii_digit() => u16::from(d - b'0'),
        Some(_) => return None,
        None => 0,
    };
    let whole = u16::try_from(parse_u32(whole)?).ok()?;
    (whole <= 10).then(|| whole * 10 + tenths)
}

fn parse_u32(text: &[u8]) -> Option<u32> {
    if text.is_empty() || text.len() > 10 {
        return None;
    }
    text.iter().try_fold(0u32, |n, &b| {
        b.is_ascii_digit().then_some(())?;
        n.checked_mul(10)?.checked_add(u32::from(b - b'0'))
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{write::GzEncoder, Compression};

    use super::*;

    const SAMPLE: &str = "tconst\taverageRating\tnumVotes\n\
        tt0133093\t8.7\t2200000\n\
        tt0000001\t5.7\t2100\n\
        tt0903747\t9.5\t2300000\r\n\
        tt9999999\t10.0\t5\n\
        garbage line\n\
        tt0000002\tN/A\t3\n";

    #[test]
    fn parses_plain_text() {
        let table = Table::parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(table.len(), 4);
        assert_eq!(table.get("tt0133093"), Some(Rating { rating: 8.7, votes: 2_200_000 }));
        assert_eq!(table.get("tt0903747"), Some(Rating { rating: 9.5, votes: 2_300_000 }));
        assert_eq!(table.get("tt9999999"), Some(Rating { rating: 10.0, votes: 5 }));
        assert_eq!(table.get("tt0000001"), Some(Rating { rating: 5.7, votes: 2100 }));
    }

    #[test]
    fn parses_gzip() {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(SAMPLE.as_bytes()).unwrap();
        let table = Table::parse(&gz.finish().unwrap()).unwrap();
        assert_eq!(table.get("tt0133093"), Some(Rating { rating: 8.7, votes: 2_200_000 }));
    }

    #[test]
    fn unknown_and_malformed_ids() {
        let table = Table::parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(table.get("tt0000002"), None);
        assert_eq!(table.get("tt1234567"), None);
        assert_eq!(table.get("0133093"), None);
        assert_eq!(table.get("tt"), None);
        assert_eq!(table.get("tt99999999999"), None);
    }

    #[test]
    fn empty_or_broken_body_is_an_error() {
        assert!(Table::parse(b"").is_err());
        assert!(Table::parse(b"tconst\taverageRating\tnumVotes\n").is_err());
        assert!(Table::parse(&[0x1f, 0x8b, 0, 0]).is_err());
    }
}
