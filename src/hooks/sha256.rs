//! SHA-256 (FIPS 180-4) over a file, for the fingerprint `tb trust` shows and re-checks.
//!
//! Written out rather than taken from a crate: tb is one static binary with no network and no
//! dependencies it does not need, and this is a FINGERPRINT — a pure function of some bytes,
//! not a secret, a key or a password hash. A pure function is exactly the thing a test can
//! pin: the three published FIPS vectors, the million-`a` vector, the empty file, and lengths
//! either side of every block and padding boundary are all asserted below, so a digest this
//! module prints is the digest every other SHA-256 prints.

use std::io::Read;
use std::path::Path;

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Streaming state: whole blocks are compressed as they arrive, so a file of any size costs
/// 64 bytes of memory.
pub struct Sha256 {
    h: [u32; 8],
    block: [u8; 64],
    used: usize,
    len: u64,
}

impl Default for Sha256 {
    fn default() -> Sha256 {
        Sha256 {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            block: [0; 64],
            used: 0,
            len: 0,
        }
    }
}

impl Sha256 {
    // `chunks_exact` over a fixed 64-byte compression block, kept as the stable, readable form
    // rather than the unstable `as_chunks::<64>()` clippy suggests.
    #[allow(clippy::chunks_exact_to_as_chunks)]
    pub fn update(&mut self, mut data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u64);
        // Top up a partial block first. If that does not complete it, `self.used` already
        // reflects the new count and there is nothing left of `data` to do anything with —
        // return here, rather than falling into the whole-block code below, which recomputes
        // `self.used` from whatever is left of `data` and would otherwise CLOBBER the count
        // just set here back down to 0 every time a byte-at-a-time `update` left a block
        // partially full (measured: an infinite loop in `finish`'s padding loop, which walks
        // `self.used` up to 56 one byte at a time and would then never arrive).
        if self.used > 0 {
            let take = (64 - self.used).min(data.len());
            self.block[self.used..self.used + take].copy_from_slice(&data[..take]);
            self.used += take;
            data = &data[take..];
            if self.used < 64 {
                return;
            }
            let b = self.block;
            self.compress(&b);
            self.used = 0;
        }
        let mut chunks = data.chunks_exact(64);
        for c in &mut chunks {
            let mut b = [0u8; 64];
            b.copy_from_slice(c);
            self.compress(&b);
        }
        let rest = chunks.remainder();
        self.block[..rest.len()].copy_from_slice(rest);
        self.used = rest.len();
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.len.wrapping_mul(8);
        self.update(&[0x80]);
        // `update` counted that byte; the length in the padding is the message's own
        self.len = self.len.wrapping_sub(1);
        while self.used != 56 {
            self.update(&[0]);
            self.len = self.len.wrapping_sub(1);
        }
        let b = bits.to_be_bytes();
        self.block[56..].copy_from_slice(&b);
        let block = self.block;
        self.compress(&block);
        let mut out = [0u8; 32];
        for (i, w) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().enumerate().take(16) {
            let c = &block[i * 4..i * 4 + 4];
            *word = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in self.h.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *x = x.wrapping_add(y);
        }
    }
}

/// Lower-case hex, the form `sha256sum` prints and `tb trust --sha256` takes.
pub fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn of_bytes(data: &[u8]) -> String {
    let mut s = Sha256::default();
    s.update(data);
    hex(&s.finish())
}

/// The digest of a file, read in 64 KiB pieces.
pub fn of_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut s = Sha256::default();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(hex(&s.finish()));
        }
        s.update(&buf[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published FIPS 180-4 vectors, plus the empty input.
    #[test]
    fn the_published_vectors() {
        assert_eq!(of_bytes(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(of_bytes(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            of_bytes(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            of_bytes(&b"a".repeat(1_000_000)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            "the million-a vector: many whole blocks, streamed"
        );
    }

    /// Every boundary the padding has: a full block, one short, one over, and the lengths
    /// where the 8-byte length field no longer fits and a second block is needed. Each digest
    /// below is the one every other SHA-256 prints for those bytes.
    #[test]
    fn lengths_around_every_block_and_padding_boundary() {
        let want = [
            (0, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            (1, "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb"),
            (55, "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318"),
            (56, "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"),
            (63, "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34"),
            (64, "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"),
            (65, "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0"),
            (119, "31eba51c313a5c08226adf18d4a359cfdfd8d2e816b13f4af952f7ea6584dcfb"),
            (128, "6836cf13bac400e9105071cd6af47084dfacad4e5e302c94bfed24e013afb73e"),
        ];
        for (n, digest) in want {
            let data = b"a".repeat(n);
            assert_eq!(of_bytes(&data), digest, "{n} bytes");
            // streamed byte by byte, and in two uneven pieces: the same digest
            let mut s = Sha256::default();
            for b in &data {
                s.update(&[*b]);
            }
            assert_eq!(hex(&s.finish()), digest, "{n} bytes, one at a time");
            let mut s = Sha256::default();
            let cut = n / 3;
            s.update(&data[..cut]);
            s.update(&data[cut..]);
            assert_eq!(hex(&s.finish()), digest, "{n} bytes, split at {cut}");
        }
    }

    #[test]
    fn a_file_hashes_like_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        for n in [0usize, 1, 64, 100_000] {
            let p = dir.path().join(format!("f{n}"));
            let data = b"x".repeat(n);
            std::fs::write(&p, &data).unwrap();
            assert_eq!(of_file(&p).unwrap(), of_bytes(&data), "{n} bytes");
        }
        // bigger than the 64 KiB read buffer, so the file path takes several reads
        assert_eq!(
            of_file(&dir.path().join("f100000")).unwrap(),
            "d69e68988157833272305aaf21f453c800346e8a3640db6578e260215542e5d4"
        );
        assert!(of_file(&dir.path().join("missing")).is_err());
    }
}
