//! Smoke test: runs `find` over a multi-line sample that mixes valid and
//! invalid instances of every kind, then `locate` over synthetic word boxes.
//!
//! `cargo run -p snap-redact --example redact`

use snap_core::geom::RectF;
use snap_core::i18n::set_language;
use snap_core::settings::RedactKind;
use snap_redact::{LineWords, find, kind_label, locate, locate_substrings};

const SAMPLE: &str = "\
Card: 4111 1111 1111 1111 (valid) and 4111 1111 1111 1112 (bad Luhn)
OCR card: 4111 l111 1111 1111
IBAN: DE89 3704 0044 0532 0130 00 ; broken DE89 3704 0044 0532 0130 01
INN: 7707083893 valid, 7707083894 invalid, 500100732259 valid12, 500100732250 invalid12
SNILS: 112-233-445 95 valid, 112-233-445 96 invalid, СНИЛС 11223344595 plain
Passport: паспорт 45 06 123456 (context)
Order 45 06 123456 ref (no context nearby, skipped)
Phones: +7 (999) 123-45-67, 8 999 123 45 67, +44 20 7946 0958, (415) 555-0132
Mail: john.doe@example.com and not-an-email@ex
IP: 192.168.1.10 valid, 192.168.1.300 invalid, IPv6 2001:db8::1, time 12:30:45
SSN: 078-05-1120 valid, 666-12-3456 invalid, 000-12-3456 invalid
AWS AKIAIOSFODNN7EXAMPLE and GitHub ghp_abcdefghijklmnopqrstuvwxyz0123456789
JWT eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abcdefghijk
password=hunter2secret and \"api_key\": \"Zx9kQ2mLp7RtW4nS8YbVc3Hd\"
Link https://user:s3cr3t@example.com/path, https://example.com/cb?token=abc123, https://example.com/docs
Random token Xk9vQ2mLp7RzT4wNs8YbVc3H vs plain abcdefghijklmnopqrstuvwxyz0123
";

fn main() {
    set_language("en");
    let kinds = RedactKind::ALL;
    let matches = find(SAMPLE, kinds);

    println!("== find(): {} matches ==", matches.len());
    for m in &matches {
        println!("{:>5}..{:<5} {:<12?} {:?}", m.start, m.end, m.kind, m.text);
    }

    println!("\n== per-kind counts ==");
    for &k in kinds {
        let n = matches.iter().filter(|m| m.kind == k).count();
        println!("{k:<12?} {n}");
    }

    let lines = vec![LineWords {
        words: vec![
            ("Card".into(), RectF::new(10.0, 10.0, 30.0, 12.0)),
            ("4111".into(), RectF::new(45.0, 10.0, 30.0, 12.0)),
            ("1111".into(), RectF::new(80.0, 10.0, 30.0, 12.0)),
            ("1111".into(), RectF::new(115.0, 10.0, 30.0, 12.0)),
            ("1111".into(), RectF::new(150.0, 10.0, 30.0, 12.0)),
            ("ok".into(), RectF::new(185.0, 10.0, 12.0, 12.0)),
        ],
    }];
    println!("\n== locate() ==");
    for hit in locate(&lines, &[RedactKind::CreditCard]) {
        println!("{:?} {:?} {:?}", hit.kind, hit.rect, hit.text);
    }
    println!("\n== locate_substrings() ==");
    for hit in locate_substrings(&lines, &["1111 1111".to_string()]) {
        println!("{:?} {:?} {:?}", hit.kind, hit.rect, hit.text);
    }

    println!("\n== labels ==");
    for &k in kinds {
        println!("en {k:<12?} {}", kind_label(k));
    }
    set_language("ru");
    for &k in kinds {
        println!("ru {k:<12?} {}", kind_label(k));
    }
}
