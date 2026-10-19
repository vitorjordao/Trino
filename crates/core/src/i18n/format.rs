//! Allocation-free formatting of a localized pattern: `{name}` and
//! `{name:kind}` placeholders filled from [`Arg`]s by the [`Locale`]'s rules.
//! Integers only (money in minor units, decimals as fixed point), so the
//! output is the same on every target.
//!
//! Kinds: none (integers grouped, text as is, money as currency), `int`
//! (grouped integer), `raw` (no grouping: years, ids), `num` (the value's
//! decimals), `currency`, `date`. `{{` and `}}` are literal braces. An
//! unknown argument writes the placeholder itself, so a QA pass sees it.

use core::fmt::Write;

use super::locale::{DateOrder, Locale};

/// A calendar date (no time zone, no validation beyond printing).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

/// An argument's value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Value<'a> {
    /// A whole number (counts: the plural form of `{n}` comes from it).
    Int(i64),
    /// Fixed point: `Dec { value: 1250, decimals: 2 }` is 12.50.
    Dec {
        value: i64,
        decimals: u8,
    },
    /// Money in minor units (cents): `Money(120000)` is 1,200.00 dollars.
    Money(i64),
    Str(&'a str),
    Date(Date),
}

/// A named argument: `Arg::int("n", 3)` fills `{n}`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Arg<'a> {
    pub name: &'a str,
    pub value: Value<'a>,
}

impl<'a> Arg<'a> {
    pub const fn int(name: &'a str, v: i64) -> Arg<'a> {
        Arg {
            name,
            value: Value::Int(v),
        }
    }

    pub const fn dec(name: &'a str, value: i64, decimals: u8) -> Arg<'a> {
        Arg {
            name,
            value: Value::Dec { value, decimals },
        }
    }

    pub const fn money(name: &'a str, minor_units: i64) -> Arg<'a> {
        Arg {
            name,
            value: Value::Money(minor_units),
        }
    }

    pub const fn str(name: &'a str, s: &'a str) -> Arg<'a> {
        Arg {
            name,
            value: Value::Str(s),
        }
    }

    pub const fn date(name: &'a str, year: u16, month: u8, day: u8) -> Arg<'a> {
        Arg {
            name,
            value: Value::Date(Date { year, month, day }),
        }
    }
}

impl Value<'_> {
    /// The number a plural form is chosen by: integer part and visible
    /// fraction digits. `None` for text and dates.
    pub fn plural_operands(&self, locale: &Locale<'_>) -> Option<(i64, u8)> {
        match *self {
            Value::Int(v) => Some((v, 0)),
            Value::Dec { value, decimals } => {
                let p = pow10(decimals);
                let frac = (value % p) != 0;
                Some((value / p, if frac { decimals } else { 0 }))
            }
            Value::Money(m) => {
                let p = pow10(locale.currency_decimals);
                Some((
                    m / p,
                    if m % p != 0 {
                        locale.currency_decimals
                    } else {
                        0
                    },
                ))
            }
            Value::Str(_) | Value::Date(_) => None,
        }
    }
}

/// The argument a plural entry counts: the one named `n` or `count`, else
/// the first number.
pub fn count_arg<'a, 'b>(args: &'b [Arg<'a>]) -> Option<&'b Arg<'a>> {
    args.iter()
        .find(|a| a.name == "n" || a.name == "count")
        .or_else(|| {
            args.iter()
                .find(|a| matches!(a.value, Value::Int(_) | Value::Dec { .. } | Value::Money(_)))
        })
}

/// Writes `pattern` with its placeholders filled.
pub fn format(
    pattern: &str,
    args: &[Arg<'_>],
    locale: &Locale<'_>,
    out: &mut dyn Write,
) -> core::fmt::Result {
    let b = pattern.as_bytes();
    let mut i = 0;
    let mut lit = 0;
    while i < b.len() {
        match b[i] {
            b'{' if b.get(i + 1) == Some(&b'{') => {
                out.write_str(&pattern[lit..i + 1])?;
                i += 2;
                lit = i;
            }
            b'}' if b.get(i + 1) == Some(&b'}') => {
                out.write_str(&pattern[lit..i + 1])?;
                i += 2;
                lit = i;
            }
            b'{' => {
                let Some(close) = pattern[i + 1..].find('}') else {
                    break;
                };
                out.write_str(&pattern[lit..i])?;
                let inner = &pattern[i + 1..i + 1 + close];
                let (name, kind) = match inner.find(':') {
                    Some(c) => (&inner[..c], &inner[c + 1..]),
                    None => (inner, ""),
                };
                match args.iter().find(|a| a.name == name) {
                    Some(a) => write_value(&a.value, kind, locale, out)?,
                    None => out.write_str(&pattern[i..i + close + 2])?,
                }
                i += close + 2;
                lit = i;
            }
            _ => i += 1,
        }
    }
    out.write_str(&pattern[lit..])
}

/// Every placeholder name in `pattern` (with its kind), in order: the
/// checks compare them across languages.
pub fn placeholders(pattern: &str, f: &mut dyn FnMut(&str, &str)) {
    let b = pattern.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' if b.get(i + 1) == Some(&b'{') => i += 2,
            b'}' if b.get(i + 1) == Some(&b'}') => i += 2,
            b'{' => {
                let Some(close) = pattern[i + 1..].find('}') else {
                    return;
                };
                let inner = &pattern[i + 1..i + 1 + close];
                match inner.find(':') {
                    Some(c) => f(&inner[..c], &inner[c + 1..]),
                    None => f(inner, ""),
                }
                i += close + 2;
            }
            _ => i += 1,
        }
    }
}

fn write_value(
    v: &Value<'_>,
    kind: &str,
    locale: &Locale<'_>,
    out: &mut dyn Write,
) -> core::fmt::Result {
    match (*v, kind) {
        (Value::Str(s), _) => out.write_str(s),
        (Value::Date(d), _) => write_date(d, locale, out),
        (Value::Money(m), "" | "currency") => write_money(m, locale, out),
        (Value::Int(i), "currency") => write_money(
            i.saturating_mul(pow10(locale.currency_decimals)),
            locale,
            out,
        ),
        (Value::Dec { value, decimals }, "currency") => write_money(
            rescale(value, decimals, locale.currency_decimals),
            locale,
            out,
        ),
        (Value::Int(i), "raw") => write_int(i, "", 1, out),
        (Value::Int(i), _) => write_int(i, locale.thousands, locale.min_group, out),
        (Value::Dec { value, decimals }, "raw") => write_dec(value, decimals, ".", "", 1, out),
        (Value::Dec { value, decimals }, _) => write_dec(
            value,
            decimals,
            locale.decimal,
            locale.thousands,
            locale.min_group,
            out,
        ),
        (Value::Money(m), _) => write_dec(
            m,
            locale.currency_decimals,
            locale.decimal,
            locale.thousands,
            locale.min_group,
            out,
        ),
    }
}

fn write_money(minor: i64, locale: &Locale<'_>, out: &mut dyn Write) -> core::fmt::Result {
    if minor < 0 {
        out.write_char('-')?;
    }
    let abs = minor.unsigned_abs().min(i64::MAX as u64) as i64;
    let space = if locale.currency_space { " " } else { "" };
    if !locale.currency_after {
        out.write_str(locale.currency)?;
        out.write_str(space)?;
    }
    write_dec(
        abs,
        locale.currency_decimals,
        locale.decimal,
        locale.thousands,
        locale.min_group,
        out,
    )?;
    if locale.currency_after {
        out.write_str(space)?;
        out.write_str(locale.currency)?;
    }
    Ok(())
}

fn write_dec(
    value: i64,
    decimals: u8,
    point: &str,
    thousands: &str,
    min_group: u8,
    out: &mut dyn Write,
) -> core::fmt::Result {
    let p = pow10(decimals) as u64;
    let abs = value.unsigned_abs();
    if value < 0 {
        out.write_char('-')?;
    }
    write_digits(abs / p, thousands, min_group, out)?;
    if decimals > 0 {
        out.write_str(point)?;
        let mut digits = [b'0'; 19];
        let mut frac = abs % p;
        for k in (0..decimals as usize).rev() {
            digits[k] = b'0' + (frac % 10) as u8;
            frac /= 10;
        }
        for &d in &digits[..decimals as usize] {
            out.write_char(d as char)?;
        }
    }
    Ok(())
}

fn write_int(v: i64, thousands: &str, min_group: u8, out: &mut dyn Write) -> core::fmt::Result {
    if v < 0 {
        out.write_char('-')?;
    }
    write_digits(v.unsigned_abs(), thousands, min_group, out)
}

/// `n` with `thousands` between groups of three, when it has at least
/// `3 + min_group` digits.
fn write_digits(n: u64, thousands: &str, min_group: u8, out: &mut dyn Write) -> core::fmt::Result {
    let mut digits = [0u8; 20];
    let mut len = 0;
    let mut m = n;
    loop {
        digits[len] = b'0' + (m % 10) as u8;
        len += 1;
        m /= 10;
        if m == 0 {
            break;
        }
    }
    let group = !thousands.is_empty() && len >= 3 + min_group.max(1) as usize;
    for k in (0..len).rev() {
        out.write_char(digits[k] as char)?;
        if group && k > 0 && k % 3 == 0 {
            out.write_str(thousands)?;
        }
    }
    Ok(())
}

fn write_date(d: Date, locale: &Locale<'_>, out: &mut dyn Write) -> core::fmt::Result {
    let two = |out: &mut dyn Write, v: u8| -> core::fmt::Result {
        out.write_char((b'0' + (v / 10) % 10) as char)?;
        out.write_char((b'0' + v % 10) as char)
    };
    let year = |out: &mut dyn Write| -> core::fmt::Result {
        let y = d.year;
        for p in [1000, 100, 10, 1] {
            out.write_char((b'0' + ((y / p) % 10) as u8) as char)?;
        }
        Ok(())
    };
    let sep = locale.date_sep;
    match locale.date_order {
        DateOrder::Dmy => {
            two(out, d.day)?;
            out.write_str(sep)?;
            two(out, d.month)?;
            out.write_str(sep)?;
            year(out)
        }
        DateOrder::Mdy => {
            two(out, d.month)?;
            out.write_str(sep)?;
            two(out, d.day)?;
            out.write_str(sep)?;
            year(out)
        }
        DateOrder::Ymd => {
            year(out)?;
            out.write_str(sep)?;
            two(out, d.month)?;
            out.write_str(sep)?;
            two(out, d.day)
        }
    }
}

fn pow10(d: u8) -> i64 {
    let mut p: i64 = 1;
    for _ in 0..d.min(18) {
        p *= 10;
    }
    p
}

fn rescale(value: i64, from: u8, to: u8) -> i64 {
    if to >= from {
        value.saturating_mul(pow10(to - from))
    } else {
        value / pow10(from - to)
    }
}
