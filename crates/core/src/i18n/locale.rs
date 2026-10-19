//! Locale rules: plural categories (CLDR) and number, currency and date
//! formats, with defaults by language tag.

/// A CLDR plural category. A plural entry stores one text per category;
/// `Other` is always present.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum PluralForm {
    Zero = 0,
    One = 1,
    Two = 2,
    Few = 3,
    Many = 4,
    Other = 5,
}

impl PluralForm {
    pub const ALL: [PluralForm; 6] = [
        PluralForm::Zero,
        PluralForm::One,
        PluralForm::Two,
        PluralForm::Few,
        PluralForm::Many,
        PluralForm::Other,
    ];

    /// `zero`, `one`, `two`, `few`, `many`, `other` (the RON field names).
    pub const fn key(self) -> &'static str {
        match self {
            PluralForm::Zero => "zero",
            PluralForm::One => "one",
            PluralForm::Two => "two",
            PluralForm::Few => "few",
            PluralForm::Many => "many",
            PluralForm::Other => "other",
        }
    }
}

/// How a language picks a [`PluralForm`] for a number: the CLDR cardinal
/// rules of the languages Trino ships, by family. `i` is the integer part,
/// `v` the count of visible fraction digits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
#[repr(u8)]
pub enum PluralRule {
    /// `one`: i = 1 and v = 0 (en, de, es, it, nl, sv, pt-PT, …).
    #[default]
    OneOther = 0,
    /// `one`: i = 0 or 1 (pt, fr): "0 convidado", "1,5 convidado".
    OneUpToOne = 1,
    /// No plural forms (ja, zh, ko, th, vi, id).
    Other = 2,
    /// ru, uk, be, hr, sr, bs: `one` 1, 21, 31…; `few` 2–4, 22–24…; `many`
    /// the other integers; `other` fractions.
    Slavic = 3,
    /// pl: `one` 1; `few` 2–4, 22–24…; `many` the other integers.
    Polish = 4,
    /// cs, sk: `one` 1; `few` 2–4; `many` fractions.
    Czech = 5,
}

impl PluralRule {
    pub const ALL: [PluralRule; 6] = [
        PluralRule::OneOther,
        PluralRule::OneUpToOne,
        PluralRule::Other,
        PluralRule::Slavic,
        PluralRule::Polish,
        PluralRule::Czech,
    ];

    pub const fn from_u8(v: u8) -> PluralRule {
        match v {
            1 => PluralRule::OneUpToOne,
            2 => PluralRule::Other,
            3 => PluralRule::Slavic,
            4 => PluralRule::Polish,
            5 => PluralRule::Czech,
            _ => PluralRule::OneOther,
        }
    }

    /// The RON name (`OneOther`, `Slavic`).
    pub const fn name(self) -> &'static str {
        match self {
            PluralRule::OneOther => "OneOther",
            PluralRule::OneUpToOne => "OneUpToOne",
            PluralRule::Other => "Other",
            PluralRule::Slavic => "Slavic",
            PluralRule::Polish => "Polish",
            PluralRule::Czech => "Czech",
        }
    }

    pub fn from_name(name: &str) -> Option<PluralRule> {
        PluralRule::ALL.into_iter().find(|r| r.name() == name)
    }

    /// The forms a table in this language must give (`other` included).
    pub const fn forms(self) -> &'static [PluralForm] {
        use PluralForm::*;
        match self {
            PluralRule::OneOther | PluralRule::OneUpToOne => &[One, Other],
            PluralRule::Other => &[Other],
            PluralRule::Slavic | PluralRule::Polish | PluralRule::Czech => &[One, Few, Many, Other],
        }
    }

    /// The form for the number `int` with `fraction_digits` visible
    /// fraction digits (`1` → 0 digits, `1.5` → 1). The sign is ignored.
    pub fn select(self, int: i64, fraction_digits: u8) -> PluralForm {
        let i = int.unsigned_abs();
        let v = fraction_digits;
        let (m10, m100) = (i % 10, i % 100);
        match self {
            PluralRule::OneOther => {
                if i == 1 && v == 0 {
                    PluralForm::One
                } else {
                    PluralForm::Other
                }
            }
            PluralRule::OneUpToOne => {
                if i <= 1 {
                    PluralForm::One
                } else {
                    PluralForm::Other
                }
            }
            PluralRule::Other => PluralForm::Other,
            PluralRule::Slavic => {
                if v != 0 {
                    PluralForm::Other
                } else if m10 == 1 && m100 != 11 {
                    PluralForm::One
                } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                    PluralForm::Few
                } else {
                    PluralForm::Many
                }
            }
            PluralRule::Polish => {
                if v != 0 {
                    PluralForm::Other
                } else if i == 1 {
                    PluralForm::One
                } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                    PluralForm::Few
                } else {
                    PluralForm::Many
                }
            }
            PluralRule::Czech => {
                if v != 0 {
                    PluralForm::Many
                } else if i == 1 {
                    PluralForm::One
                } else if (2..=4).contains(&i) {
                    PluralForm::Few
                } else {
                    PluralForm::Other
                }
            }
        }
    }

    /// The default rule for a language tag, by its primary subtag
    /// (`pt-PT` is the one region with its own rule).
    pub fn for_tag(tag: &str) -> PluralRule {
        let primary = primary(tag);
        if eq_ci(tag, "pt-PT") {
            return PluralRule::OneOther;
        }
        match () {
            _ if one_of(primary, &["fr", "pt", "hy", "ff", "kab"]) => PluralRule::OneUpToOne,
            _ if one_of(
                primary,
                &["ja", "zh", "ko", "th", "vi", "id", "ms", "lo", "my", "km"],
            ) =>
            {
                PluralRule::Other
            }
            _ if one_of(primary, &["ru", "uk", "be", "hr", "sr", "bs", "sh"]) => PluralRule::Slavic,
            _ if one_of(primary, &["pl"]) => PluralRule::Polish,
            _ if one_of(primary, &["cs", "sk"]) => PluralRule::Czech,
            _ => PluralRule::OneOther,
        }
    }
}

/// The order a date's parts are written in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
#[repr(u8)]
pub enum DateOrder {
    /// 31/12/2026 (most of the world).
    #[default]
    Dmy = 0,
    /// 12/31/2026 (en-US).
    Mdy = 1,
    /// 2026/12/31 (ja, zh, ko, ISO).
    Ymd = 2,
}

impl DateOrder {
    pub const fn from_u8(v: u8) -> DateOrder {
        match v {
            1 => DateOrder::Mdy,
            2 => DateOrder::Ymd,
            _ => DateOrder::Dmy,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            DateOrder::Dmy => "Dmy",
            DateOrder::Mdy => "Mdy",
            DateOrder::Ymd => "Ymd",
        }
    }

    pub fn from_name(name: &str) -> Option<DateOrder> {
        [DateOrder::Dmy, DateOrder::Mdy, DateOrder::Ymd]
            .into_iter()
            .find(|d| d.name() == name)
    }
}

/// How a language writes numbers, money and dates. Part of every baked
/// string table, so a game formats without knowing the locale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Locale<'a> {
    /// Between the integer and the fraction: `.` (en) or `,` (pt, es).
    pub decimal: &'a str,
    /// Between groups of three digits: `,` (en), `.` (pt, es), ` ` (fr).
    pub thousands: &'a str,
    /// The currency symbol (`$`, `R$`, `€`).
    pub currency: &'a str,
    /// The symbol follows the number (`1.200,00 €`).
    pub currency_after: bool,
    /// A space between the symbol and the number (`R$ 1.200,00`).
    pub currency_space: bool,
    /// Fraction digits of a currency amount (2; 0 for the yen).
    pub currency_decimals: u8,
    /// Group only numbers with at least this many integer digits above 3:
    /// 1 groups `1,200`; 2 (es, pl) writes `1200` but `12.000`.
    pub min_group: u8,
    pub date_order: DateOrder,
    pub date_sep: &'a str,
    pub plural: PluralRule,
}

impl Locale<'static> {
    /// en-US: `1,200.50`, `$1,200.00`, `12/31/2026`.
    pub const EN_US: Locale<'static> = Locale {
        decimal: ".",
        thousands: ",",
        currency: "$",
        currency_after: false,
        currency_space: false,
        currency_decimals: 2,
        min_group: 1,
        date_order: DateOrder::Mdy,
        date_sep: "/",
        plural: PluralRule::OneOther,
    };

    /// The defaults for a language tag (`pt-BR`, `es`, `ja-JP`); unknown
    /// tags get en-US's numbers with their own plural rule. A table's
    /// `locale` overrides any field.
    pub fn for_tag(tag: &str) -> Locale<'static> {
        let p = primary(tag);
        let plural = PluralRule::for_tag(tag);
        let base = Locale {
            plural,
            ..Locale::EN_US
        };
        // Decimal comma, dot groups, € after: the euro zone's default.
        let euro = Locale {
            decimal: ",",
            thousands: ".",
            currency: "€",
            currency_after: true,
            currency_space: true,
            date_order: DateOrder::Dmy,
            ..base
        };
        if eq_ci(tag, "pt-PT") {
            return Locale {
                thousands: " ",
                min_group: 2,
                ..euro
            };
        }
        match p {
            _ if eq_ci(p, "en") => {
                if eq_ci(tag, "en-GB") || eq_ci(tag, "en-IE") {
                    Locale {
                        currency: if eq_ci(tag, "en-GB") { "£" } else { "€" },
                        date_order: DateOrder::Dmy,
                        ..base
                    }
                } else {
                    base
                }
            }
            _ if eq_ci(p, "pt") => Locale {
                currency: "R$",
                currency_after: false,
                ..euro
            },
            _ if eq_ci(p, "es") => {
                if region(tag).is_empty() || eq_ci(region(tag), "ES") {
                    Locale {
                        min_group: 2,
                        ..euro
                    }
                } else {
                    // Latin America: dot decimals, $ before.
                    Locale {
                        date_order: DateOrder::Dmy,
                        ..base
                    }
                }
            }
            _ if eq_ci(p, "fr") => Locale {
                thousands: " ",
                ..euro
            },
            _ if eq_ci(p, "de") => Locale {
                date_sep: ".",
                ..euro
            },
            _ if eq_ci(p, "it") => euro,
            _ if eq_ci(p, "nl") => Locale {
                currency_after: false,
                date_sep: "-",
                ..euro
            },
            _ if eq_ci(p, "ru") || eq_ci(p, "uk") => Locale {
                thousands: " ",
                currency: if eq_ci(p, "ru") { "₽" } else { "₴" },
                date_sep: ".",
                ..euro
            },
            _ if eq_ci(p, "pl") => Locale {
                thousands: " ",
                currency: "zł",
                min_group: 2,
                date_sep: ".",
                ..euro
            },
            _ if eq_ci(p, "ja") || eq_ci(p, "zh") => Locale {
                currency: "¥",
                currency_decimals: if eq_ci(p, "ja") { 0 } else { 2 },
                date_order: DateOrder::Ymd,
                ..base
            },
            _ if eq_ci(p, "ko") => Locale {
                currency: "₩",
                currency_decimals: 0,
                date_order: DateOrder::Ymd,
                date_sep: ".",
                ..base
            },
            _ => base,
        }
    }
}

/// `pt` of `pt-BR` (the whole tag when it has no region).
pub fn primary(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or(tag)
}

/// `BR` of `pt-BR` (empty without one).
pub fn region(tag: &str) -> &str {
    let mut parts = tag.split(['-', '_']);
    let _ = parts.next();
    parts.next().unwrap_or("")
}

/// ASCII case-insensitive equality (tags are ASCII).
pub fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn one_of(primary: &str, list: &[&str]) -> bool {
    list.iter().any(|l| eq_ci(primary, l))
}
