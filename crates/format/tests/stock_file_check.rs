//! docs/specs/stock-snapshot-restore AC-5: a copied file's length and
//! Lucene footer CRC are checked as its bytes stream past.

use oss_format::StockFileCheck;

/// A file of `body` bytes with a Lucene footer, and its CRC.
fn lucene(body: &[u8]) -> (Vec<u8>, u32) {
    let mut file = body.to_vec();
    file.extend_from_slice(&[0xC0, 0x28, 0x93, 0xE8, 0, 0, 0, 0]);
    let crc = crc32fast::hash(&file);
    file.extend_from_slice(&u64::from(crc).to_be_bytes());
    (file, crc)
}

fn base36(mut n: u64) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[usize::try_from(n % 36).unwrap()]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

fn run(file: &[u8], listed: &str, step: usize) -> Result<(), oss_format::FormatError> {
    let mut check = StockFileCheck::new(file.len() as u64, listed)?;
    for piece in file.chunks(step) {
        check.update(piece)?;
    }
    check.finish()
}

#[test]
fn a_whole_file_passes_at_any_step() {
    let (file, crc) = lucene(&(0..200u8).collect::<Vec<_>>());
    let listed = base36(u64::from(crc));
    for step in [1, 3, 7, 8, 15, 16, 17, 100, 216, 1000] {
        run(&file, &listed, step).unwrap_or_else(|e| panic!("step {step}: {e}"));
    }
    let (empty, crc) = lucene(b"");
    run(&empty, &base36(u64::from(crc)), 5).expect("a footer alone");
}

#[test]
fn every_flipped_byte_is_refused() {
    let (file, crc) = lucene(b"some bytes of a segment file");
    let listed = base36(u64::from(crc));
    for i in 0..file.len() {
        let mut bad = file.clone();
        bad[i] ^= 1;
        assert!(run(&bad, &listed, 5).is_err(), "byte {i}");
    }
}

#[test]
fn a_wrong_length_or_listing_is_refused() {
    let (file, crc) = lucene(b"abcdefgh");
    let listed = base36(u64::from(crc));
    // Short: every truncation.
    for n in 0..file.len() {
        let mut check = StockFileCheck::new(file.len() as u64, &listed).unwrap();
        check.update(&file[..n]).unwrap();
        assert!(check.finish().is_err(), "short {n}");
    }
    // Long: a byte past the length.
    let mut check = StockFileCheck::new(file.len() as u64, &listed).unwrap();
    check.update(&file).unwrap();
    assert!(check.update(b"x").is_err());
    // A different listed checksum, and one that is not a number.
    assert!(run(&file, &base36(u64::from(crc) ^ 1), 4).is_err());
    assert!(StockFileCheck::new(file.len() as u64, "!").is_err());
    assert!(StockFileCheck::new(file.len() as u64, &base36(1 << 33)).is_err());
    // Shorter than a footer.
    assert!(StockFileCheck::new(15, "0").is_err());
}

#[test]
fn a_reset_starts_over() {
    let (file, crc) = lucene(b"retry me");
    let mut check = StockFileCheck::new(file.len() as u64, &base36(u64::from(crc))).unwrap();
    check.update(&file[..10]).unwrap();
    check.reset();
    check.update(&file).unwrap();
    check.finish().expect("whole after a reset");
}

#[test]
fn a_bad_magic_or_algorithm_is_refused() {
    let (mut file, _) = lucene(b"x");
    let at = file.len() - 16;
    file[at] ^= 1;
    let crc = crc32fast::hash(&file[..file.len() - 8]);
    let n = file.len();
    file[n - 8..].copy_from_slice(&u64::from(crc).to_be_bytes());
    assert!(run(&file, &base36(u64::from(crc)), 4).is_err());
    let (mut file, _) = lucene(b"x");
    let n = file.len();
    file[n - 9] = 1;
    let crc = crc32fast::hash(&file[..n - 8]);
    file[n - 8..].copy_from_slice(&u64::from(crc).to_be_bytes());
    assert!(run(&file, &base36(u64::from(crc)), 4).is_err());
}
