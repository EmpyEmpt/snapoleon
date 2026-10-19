//! Checksum and structure validators. Inputs are ASCII digit strings unless
//! stated otherwise; callers strip separators first.

/// Luhn mod-10 check over an ASCII digit string.
pub fn luhn(digits: &[u8]) -> bool {
    let mut sum = 0u32;
    for (i, &c) in digits.iter().rev().enumerate() {
        let mut d = u32::from(c - b'0');
        if i % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    sum.is_multiple_of(10)
}

/// Whether the leading digits belong to a known payment-card network (IIN/BIN range).
pub fn known_card_iin(digits: &[u8]) -> bool {
    let num = |k: usize| {
        digits
            .iter()
            .take(k)
            .fold(0u32, |a, &c| a * 10 + u32::from(c - b'0'))
    };
    let (p2, p3, p4, p6) = (num(2), num(3), num(4), num(6));
    digits.first() == Some(&b'4')
        || matches!(
            p2,
            34 | 36 | 37 | 38 | 39 | 50 | 51..=58 | 62 | 63 | 65 | 67
        )
        || (2200..=2204).contains(&p4)
        || (2221..=2720).contains(&p4)
        || (300..=305).contains(&p3)
        || (644..=649).contains(&p3)
        || (3528..=3589).contains(&p4)
        || p4 == 6011
        || (622_126..=622_925).contains(&p6)
}

/// ISO 13616 mod-97 check. Input is the whole IBAN with spaces removed.
pub fn iban_valid(iban: &str) -> bool {
    let b = iban.as_bytes();
    if !(15..=34).contains(&b.len()) || !b[..2].iter().all(u8::is_ascii_uppercase) {
        return false;
    }
    if !b[2..4].iter().all(u8::is_ascii_digit) {
        return false;
    }
    let mut rem = 0u32;
    for &c in b[4..].iter().chain(&b[..4]) {
        rem = match c {
            b'0'..=b'9' => (rem * 10 + u32::from(c - b'0')) % 97,
            b'A'..=b'Z' => (rem * 100 + u32::from(c - b'A') + 10) % 97,
            _ => return false,
        };
    }
    rem == 1
}

/// Russian taxpayer identification number: 10 digits (legal entity) or 12 (person).
pub fn inn_valid(d: &[u8]) -> bool {
    let v: Vec<u32> = d.iter().map(|&c| u32::from(c - b'0')).collect();
    let check = |n: &[u32], w: &[u32]| n.iter().zip(w).map(|(a, b)| a * b).sum::<u32>() % 11 % 10;
    match v.len() {
        10 => check(&v[..9], &[2, 4, 10, 3, 5, 9, 4, 6, 8]) == v[9],
        12 => {
            check(&v[..10], &[7, 2, 4, 10, 3, 5, 9, 4, 6, 8]) == v[10]
                && check(&v[..11], &[3, 7, 2, 4, 10, 3, 5, 9, 4, 6, 8]) == v[11]
        }
        _ => false,
    }
}

/// Russian insurance number (СНИЛС): 11 digits, two trailing check digits.
pub fn snils_valid(d: &[u8]) -> bool {
    if d.len() != 11 {
        return false;
    }
    let v: Vec<u32> = d.iter().map(|&c| u32::from(c - b'0')).collect();
    let sum: u32 = v[..9]
        .iter()
        .enumerate()
        .map(|(i, &x)| x * (9 - i as u32))
        .sum();
    let check = match sum {
        0..=99 => sum,
        100 | 101 => 0,
        _ => match sum % 101 {
            100 => 0,
            r => r,
        },
    };
    check == v[9] * 10 + v[10]
}

/// US SSN structure: area not 000/666/9xx, group not 00, serial not 0000.
pub fn ssn_valid(area: &str, group: &str, serial: &str) -> bool {
    area != "000" && area != "666" && !area.starts_with('9') && group != "00" && serial != "0000"
}

/// Shannon entropy in bits per character.
pub fn entropy(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    for &b in s.as_bytes() {
        counts[usize::from(b)] += 1;
    }
    let n = s.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = f64::from(c) / n;
            -p * p.log2()
        })
        .sum()
}
