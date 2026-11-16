//! Formatação centralizada de tamanhos, números e datas (pt-BR).

/// "1,50 MB", "512 B"… (base 1024, vírgula decimal).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    let s = if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    };
    format!("{} {}", s.replace('.', ","), UNITS[u])
}

/// Inteiro com separador de milhar ".".
pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    out
}

/// Percentual com uma casa e vírgula ("12,3%").
pub fn format_percent(fraction: f64) -> String {
    format!("{:.1}%", fraction * 100.0).replace('.', ",")
}

const FILETIME_UNIX_DIFF_SECS: i64 = 11_644_473_600;

/// FILETIME → "dd/mm/aaaa hh:mm" (UTC). Vazio se zero/inválido.
pub fn format_filetime(ft: i64) -> String {
    if ft <= 0 {
        return String::new();
    }
    let secs = ft / 10_000_000 - FILETIME_UNIX_DIFF_SECS;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{d:02}/{m:02}/{y:04} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// FILETIME → "aaaa-mm-dd hh:mm:ss" (UTC). Vazio se zero/inválido.
pub fn format_filetime_iso(ft: i64) -> String {
    if ft <= 0 {
        return String::new();
    }
    let secs = ft / 10_000_000 - FILETIME_UNIX_DIFF_SECS;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Algoritmo de Howard Hinnant (dias desde 1970-01-01 → data civil).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
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
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1536), "1,50 KB");
        assert_eq!(format_size(10 * 1024 * 1024), "10,0 MB");
        assert_eq!(format_size(150 * 1024 * 1024 * 1024), "150 GB");
    }

    #[test]
    fn counts() {
        assert_eq!(format_count(7), "7");
        assert_eq!(format_count(1000), "1.000");
        assert_eq!(format_count(1_234_567), "1.234.567");
    }

    #[test]
    fn percent() {
        assert_eq!(format_percent(0.1234), "12,3%");
    }

    #[test]
    fn filetime() {
        // 2024-01-02 03:04:00 UTC
        let unix = 1_704_164_640i64;
        let ft = (unix + FILETIME_UNIX_DIFF_SECS) * 10_000_000;
        assert_eq!(format_filetime(ft), "02/01/2024 03:04");
        assert_eq!(format_filetime(0), "");
    }
}
