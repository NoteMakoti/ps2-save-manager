//! The PS2 "time of day" timestamp stored in directory entries.
//!
//! Eight bytes: `unused, sec, min, hour, day, month, year(u16 LE)`.
//! The console keeps these in Japan Standard Time (UTC+9).

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Tod {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub min: u8,
    pub sec: u8,
}

const JST_OFFSET: i64 = 9 * 3600;

impl Tod {
    pub fn from_bytes(b: &[u8]) -> Tod {
        Tod {
            sec: b[1],
            min: b[2],
            hour: b[3],
            day: b[4],
            month: b[5],
            year: u16::from_le_bytes([b[6], b[7]]),
        }
    }

    pub fn to_bytes(&self) -> [u8; 8] {
        let y = self.year.to_le_bytes();
        [0, self.sec, self.min, self.hour, self.day, self.month, y[0], y[1]]
    }

    /// Current time, in the console's (JST) time base.
    pub fn now() -> Tod {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Tod::from_unix(secs)
    }

    /// Convert a Unix timestamp (UTC) to a PS2 (JST) timestamp.
    pub fn from_unix(secs: i64) -> Tod {
        let t = secs + JST_OFFSET;
        let days = t.div_euclid(86400);
        let rem = t.rem_euclid(86400);
        let (y, m, d) = civil_from_days(days);
        Tod {
            year: y as u16,
            month: m as u8,
            day: d as u8,
            hour: (rem / 3600) as u8,
            min: ((rem % 3600) / 60) as u8,
            sec: (rem % 60) as u8,
        }
    }

    /// Convert to a Unix timestamp (UTC). Invalid dates yield 0.
    pub fn to_unix(&self) -> i64 {
        if self.year == 0 {
            return 0;
        }
        let month = self.month.clamp(1, 12) as i64;
        let day = self.day.clamp(1, 31) as i64;
        let days = days_from_civil(self.year as i64, month, day);
        days * 86400 + self.hour as i64 * 3600 + self.min as i64 * 60 + self.sec as i64
            - JST_OFFSET
    }

    pub fn is_zero(&self) -> bool {
        self.year == 0
    }
}

impl std::fmt::Display for Tod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.min, self.sec
        )
    }
}

// Howard Hinnant's civil date algorithms.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        for s in [0i64, 1_000_000_000, 1_700_000_000, 951_782_400] {
            assert_eq!(Tod::from_unix(s).to_unix(), s);
        }
        let t = Tod::from_unix(0);
        assert_eq!((t.year, t.month, t.day, t.hour), (1970, 1, 1, 9));
        let b = t.to_bytes();
        assert_eq!(Tod::from_bytes(&b), t);
    }
}
