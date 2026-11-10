//! LZARI (LZSS + adaptive arithmetic coding), Haruhiko Okumura 1989,
//! as used by Action Replay MAX (`.max`) saves.
//!
//! This is a direct port of the original public-domain LZARI.C so the
//! output is bit-compatible with files produced by the MAX software.

const N: usize = 4096; // ring buffer size
const F: usize = 60; // lookahead size
const THRESHOLD: usize = 2;
const NIL: usize = N;

const M: u32 = 15;
const Q1: u32 = 1 << M;
const Q2: u32 = 2 * Q1;
const Q3: u32 = 3 * Q1;
const Q4: u32 = 4 * Q1;
const MAX_CUM: u32 = Q1 - 1;
const N_CHAR: usize = 256 - THRESHOLD + F;

struct Model {
    char_to_sym: [usize; N_CHAR],
    sym_to_char: [usize; N_CHAR + 1],
    sym_freq: [u32; N_CHAR + 1],
    sym_cum: [u32; N_CHAR + 1],
    position_cum: Vec<u32>,
}

impl Model {
    fn new() -> Model {
        let mut m = Model {
            char_to_sym: [0; N_CHAR],
            sym_to_char: [0; N_CHAR + 1],
            sym_freq: [0; N_CHAR + 1],
            sym_cum: [0; N_CHAR + 1],
            position_cum: vec![0; N + 1],
        };
        m.sym_cum[N_CHAR] = 0;
        for sym in (1..=N_CHAR).rev() {
            let ch = sym - 1;
            m.char_to_sym[ch] = sym;
            m.sym_to_char[sym] = ch;
            m.sym_freq[sym] = 1;
            m.sym_cum[sym - 1] = m.sym_cum[sym] + m.sym_freq[sym];
        }
        m.sym_freq[0] = 0;
        m.position_cum[N] = 0;
        for i in (1..=N).rev() {
            m.position_cum[i - 1] = m.position_cum[i] + 10000 / (i as u32 + 200);
        }
        m
    }

    fn update(&mut self, sym: usize) {
        if self.sym_cum[0] >= MAX_CUM {
            let mut c = 0;
            for i in (1..=N_CHAR).rev() {
                self.sym_cum[i] = c;
                self.sym_freq[i] = (self.sym_freq[i] + 1) >> 1;
                c += self.sym_freq[i];
            }
            self.sym_cum[0] = c;
        }
        let mut i = sym;
        while self.sym_freq[i] == self.sym_freq[i - 1] {
            i -= 1;
        }
        if i < sym {
            let ch_i = self.sym_to_char[i];
            let ch_sym = self.sym_to_char[sym];
            self.sym_to_char[i] = ch_sym;
            self.sym_to_char[sym] = ch_i;
            self.char_to_sym[ch_i] = sym;
            self.char_to_sym[ch_sym] = i;
        }
        self.sym_freq[i] += 1;
        loop {
            if i == 0 {
                break;
            }
            i -= 1;
            self.sym_cum[i] += 1;
        }
    }
}

// ---------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------

struct Encoder {
    text_buf: Vec<u8>,
    lson: Vec<usize>,
    rson: Vec<usize>,
    dad: Vec<usize>,
    match_position: usize,
    match_length: usize,
    low: u32,
    high: u32,
    shifts: u32,
    out: Vec<u8>,
    bit_buf: u8,
    bit_mask: u8,
    model: Model,
}

impl Encoder {
    fn put_bit(&mut self, bit: bool) {
        if bit {
            self.bit_buf |= self.bit_mask;
        }
        self.bit_mask >>= 1;
        if self.bit_mask == 0 {
            self.out.push(self.bit_buf);
            self.bit_buf = 0;
            self.bit_mask = 128;
        }
    }

    fn flush_bits(&mut self) {
        for _ in 0..7 {
            self.put_bit(false);
        }
    }

    fn output(&mut self, bit: bool) {
        self.put_bit(bit);
        while self.shifts > 0 {
            self.put_bit(!bit);
            self.shifts -= 1;
        }
    }

    fn init_tree(&mut self) {
        for i in N + 1..=N + 256 {
            self.rson[i] = NIL;
        }
        for i in 0..N {
            self.dad[i] = NIL;
        }
    }

    fn insert_node(&mut self, r: usize) {
        let mut cmp: i32 = 1;
        let mut p = N + 1 + self.text_buf[r] as usize;
        self.rson[r] = NIL;
        self.lson[r] = NIL;
        self.match_length = 0;
        loop {
            if cmp >= 0 {
                if self.rson[p] != NIL {
                    p = self.rson[p];
                } else {
                    self.rson[p] = r;
                    self.dad[r] = p;
                    return;
                }
            } else if self.lson[p] != NIL {
                p = self.lson[p];
            } else {
                self.lson[p] = r;
                self.dad[r] = p;
                return;
            }
            let mut i = 1;
            while i < F {
                cmp = self.text_buf[r + i] as i32 - self.text_buf[p + i] as i32;
                if cmp != 0 {
                    break;
                }
                i += 1;
            }
            if i > THRESHOLD {
                if i > self.match_length {
                    self.match_position = (r.wrapping_sub(p)) & (N - 1);
                    self.match_length = i;
                    if i >= F {
                        break;
                    }
                } else if i == self.match_length {
                    let temp = (r.wrapping_sub(p)) & (N - 1);
                    if temp < self.match_position {
                        self.match_position = temp;
                    }
                }
            }
        }
        self.dad[r] = self.dad[p];
        self.lson[r] = self.lson[p];
        self.rson[r] = self.rson[p];
        let lp = self.lson[p];
        let rp = self.rson[p];
        self.dad[lp] = r;
        self.dad[rp] = r;
        let dp = self.dad[p];
        if self.rson[dp] == p {
            self.rson[dp] = r;
        } else {
            self.lson[dp] = r;
        }
        self.dad[p] = NIL;
    }

    fn delete_node(&mut self, p: usize) {
        if self.dad[p] == NIL {
            return;
        }
        let q;
        if self.rson[p] == NIL {
            q = self.lson[p];
        } else if self.lson[p] == NIL {
            q = self.rson[p];
        } else {
            let mut qq = self.lson[p];
            if self.rson[qq] != NIL {
                loop {
                    qq = self.rson[qq];
                    if self.rson[qq] == NIL {
                        break;
                    }
                }
                let dq = self.dad[qq];
                let lq = self.lson[qq];
                self.rson[dq] = lq;
                self.dad[lq] = dq;
                self.lson[qq] = self.lson[p];
                let lp = self.lson[p];
                self.dad[lp] = qq;
            }
            self.rson[qq] = self.rson[p];
            let rp = self.rson[p];
            self.dad[rp] = qq;
            q = qq;
        }
        self.dad[q] = self.dad[p];
        let dp = self.dad[p];
        if self.rson[dp] == p {
            self.rson[dp] = q;
        } else {
            self.lson[dp] = q;
        }
        self.dad[p] = NIL;
    }

    fn encode_interval(&mut self, lo_num: u32, hi_num: u32, total: u32) {
        let range = (self.high - self.low) as u64;
        self.high = self.low + (range * hi_num as u64 / total as u64) as u32;
        self.low += (range * lo_num as u64 / total as u64) as u32;
        loop {
            if self.high <= Q2 {
                self.output(false);
            } else if self.low >= Q2 {
                self.output(true);
                self.low -= Q2;
                self.high -= Q2;
            } else if self.low >= Q1 && self.high <= Q3 {
                self.shifts += 1;
                self.low -= Q1;
                self.high -= Q1;
            } else {
                break;
            }
            self.low += self.low;
            self.high += self.high;
        }
    }

    fn encode_char(&mut self, ch: usize) {
        let sym = self.model.char_to_sym[ch];
        let (hi, lo, tot) = (self.model.sym_cum[sym - 1], self.model.sym_cum[sym], self.model.sym_cum[0]);
        self.encode_interval(lo, hi, tot);
        self.model.update(sym);
    }

    fn encode_position(&mut self, position: usize) {
        let (hi, lo, tot) = (
            self.model.position_cum[position],
            self.model.position_cum[position + 1],
            self.model.position_cum[0],
        );
        self.encode_interval(lo, hi, tot);
    }

    fn encode_end(&mut self) {
        self.shifts += 1;
        if self.low < Q1 {
            self.output(false);
        } else {
            self.output(true);
        }
        self.flush_bits();
    }
}

/// Compress `input` with LZARI.
pub fn encode(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut e = Encoder {
        text_buf: vec![0u8; N + F - 1],
        lson: vec![0; N + 1],
        rson: vec![0; N + 257],
        dad: vec![0; N + 1],
        match_position: 0,
        match_length: 0,
        low: 0,
        high: Q4,
        shifts: 0,
        out: Vec::with_capacity(input.len() / 2),
        bit_buf: 0,
        bit_mask: 128,
        model: Model::new(),
    };
    e.init_tree();
    let mut inp = input.iter().copied();
    let mut s = 0usize;
    let mut r = N - F;
    for i in s..r {
        e.text_buf[i] = b' ';
    }
    let mut len = 0usize;
    while len < F {
        match inp.next() {
            Some(c) => {
                e.text_buf[r + len] = c;
                len += 1;
            }
            None => break,
        }
    }
    for i in 1..=F {
        e.insert_node(r - i);
    }
    e.insert_node(r);
    loop {
        if e.match_length > len {
            e.match_length = len;
        }
        if e.match_length <= THRESHOLD {
            e.match_length = 1;
            let ch = e.text_buf[r] as usize;
            e.encode_char(ch);
        } else {
            e.encode_char(255 - THRESHOLD + e.match_length);
            let p = e.match_position;
            e.encode_position(p - 1);
        }
        let last_match_length = e.match_length;
        let mut i = 0;
        while i < last_match_length {
            let c = match inp.next() {
                Some(c) => c,
                None => break,
            };
            e.delete_node(s);
            e.text_buf[s] = c;
            if s < F - 1 {
                e.text_buf[s + N] = c;
            }
            s = (s + 1) & (N - 1);
            r = (r + 1) & (N - 1);
            e.insert_node(r);
            i += 1;
        }
        while i < last_match_length {
            i += 1;
            e.delete_node(s);
            s = (s + 1) & (N - 1);
            r = (r + 1) & (N - 1);
            len -= 1;
            if len > 0 {
                e.insert_node(r);
            }
        }
        if len == 0 {
            break;
        }
    }
    e.encode_end();
    e.out
}

// ---------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    mask: u8,
}

impl BitReader<'_> {
    fn get_bit(&mut self) -> u32 {
        if self.pos >= self.data.len() {
            return 0;
        }
        let bit = (self.data[self.pos] & self.mask != 0) as u32;
        self.mask >>= 1;
        if self.mask == 0 {
            self.mask = 128;
            self.pos += 1;
        }
        bit
    }
}

/// Decompress LZARI data into exactly `out_len` bytes.
pub fn decode(input: &[u8], out_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(out_len);
    if out_len == 0 {
        return out;
    }
    let mut model = Model::new();
    let mut bits = BitReader { data: input, pos: 0, mask: 128 };
    let mut text_buf = vec![0u8; N + F - 1];
    for b in text_buf.iter_mut().take(N - F) {
        *b = b' ';
    }
    let mut r = N - F;
    let mut low: u32 = 0;
    let mut high: u32 = Q4;
    let mut value: u32 = 0;
    for _ in 0..M + 2 {
        value = 2 * value + bits.get_bit();
    }

    // shared renormalisation
    macro_rules! renorm {
        () => {
            loop {
                if low >= Q2 {
                    value -= Q2;
                    low -= Q2;
                    high -= Q2;
                } else if low >= Q1 && high <= Q3 {
                    value -= Q1;
                    low -= Q1;
                    high -= Q1;
                } else if high > Q2 {
                    break;
                }
                low += low;
                high += high;
                value = 2 * value + bits.get_bit();
            }
        };
    }

    while out.len() < out_len {
        // DecodeChar
        let range = (high - low) as u64;
        let x = (((value - low + 1) as u64 * model.sym_cum[0] as u64 - 1) / range) as u32;
        let (mut i, mut j) = (1usize, N_CHAR);
        while i < j {
            let k = (i + j) / 2;
            if model.sym_cum[k] > x {
                i = k + 1;
            } else {
                j = k;
            }
        }
        let sym = i;
        high = low + (range * model.sym_cum[sym - 1] as u64 / model.sym_cum[0] as u64) as u32;
        low += (range * model.sym_cum[sym] as u64 / model.sym_cum[0] as u64) as u32;
        renorm!();
        let c = model.sym_to_char[sym];
        model.update(sym);

        if c < 256 {
            out.push(c as u8);
            text_buf[r] = c as u8;
            r = (r + 1) & (N - 1);
        } else {
            // DecodePosition
            let range = (high - low) as u64;
            let pc = &model.position_cum;
            let x = (((value - low + 1) as u64 * pc[0] as u64 - 1) / range) as u32;
            let (mut i, mut j) = (1usize, N);
            while i < j {
                let k = (i + j) / 2;
                if pc[k] > x {
                    i = k + 1;
                } else {
                    j = k;
                }
            }
            let position = i - 1;
            high = low + (range * pc[position] as u64 / pc[0] as u64) as u32;
            low += (range * pc[position + 1] as u64 / pc[0] as u64) as u32;
            renorm!();

            let start = (r.wrapping_sub(position).wrapping_sub(1)) & (N - 1);
            let j = c - 255 + THRESHOLD;
            for k in 0..j {
                if out.len() >= out_len {
                    break;
                }
                let ch = text_buf[(start + k) & (N - 1)];
                out.push(ch);
                text_buf[r] = ch;
                r = (r + 1) & (N - 1);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(data: &[u8]) {
        let c = encode(data);
        let d = decode(&c, data.len());
        assert_eq!(d, data);
    }

    #[test]
    fn roundtrips() {
        rt(b"");
        rt(b"a");
        rt(b"hello hello hello hello world");
        rt(&vec![0u8; 100_000]);
        let mut x: u32 = 12345;
        let noise: Vec<u8> = (0..70_000)
            .map(|_| {
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                (x >> 16) as u8
            })
            .collect();
        rt(&noise);
        let text: Vec<u8> = (0..50_000).map(|i| b"the quick brown fox "[i % 20] ^ ((i / 997) as u8 & 3)).collect();
        rt(&text);
    }
}
