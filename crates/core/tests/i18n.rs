//! Localization through the public API (issue #59): TSTR v2 and v1, the
//! fallback chain, plural rules, locale formatting (pt-BR, en-US and es-ES
//! as fixtures), language matching and the UI's per-language fonts.

use trino_core::i18n::table::{EntrySpec, TableSpec, encode_tstr_with};
use trino_core::i18n::{
    Arg, DateOrder, Entry, LangId, LangTag, Locale, Localizer, PluralForm, PluralRule, StrKey,
    StringTable, StringsError,
};
use trino_core::render::FontId;
use trino_core::ui::UiStrings;
use trino_core::{GroupId, TextBuf};

fn bake(spec: &TableSpec<'_>, entries: &[(&str, EntrySpec<'_>)]) -> &'static [u8] {
    let mut e = entries.to_vec();
    let mut out = Vec::new();
    encode_tstr_with(spec, &mut e, &mut |b| out.extend_from_slice(b)).unwrap();
    Box::leak(out.into_boxed_slice())
}

fn plural(one: &'static str, other: &'static str) -> EntrySpec<'static> {
    EntrySpec::Plural(["", one, "", "", "", other])
}

fn table(
    lang: &'static str,
    fallback: &'static str,
    entries: &[(&str, EntrySpec<'_>)],
) -> StringTable<'static> {
    let spec = TableSpec {
        name: match lang {
            "pt-BR" => "Português (Brasil)",
            "es-ES" => "Español",
            _ => "English",
        },
        ..TableSpec::plain(lang, fallback)
    };
    StringTable::parse(bake(&spec, entries)).unwrap()
}

fn fixture() -> Localizer<'static, 4> {
    let en = table(
        "en-US",
        "",
        &[
            ("menu.play", EntrySpec::Text("Play")),
            ("menu.quit", EntrySpec::Text("Quit")),
            ("guests.count", plural("{n} guest", "{n} guests")),
            ("money", EntrySpec::Text("{amount:currency}")),
            ("score", EntrySpec::Text("Score: {points}")),
            ("born", EntrySpec::Text("Born {d:date}")),
        ],
    );
    let pt = table(
        "pt-BR",
        "en-US",
        &[
            ("menu.play", EntrySpec::Text("Jogar")),
            ("guests.count", plural("{n} convidado", "{n} convidados")),
            ("money", EntrySpec::Text("{amount:currency}")),
            ("score", EntrySpec::Text("Pontos: {points}")),
            ("born", EntrySpec::Text("Nasceu em {d:date}")),
        ],
    );
    let es = table(
        "es-ES",
        "en-US",
        &[
            ("menu.play", EntrySpec::Text("Jugar")),
            ("guests.count", plural("{n} invitado", "{n} invitados")),
            ("money", EntrySpec::Text("{amount:currency}")),
            ("score", EntrySpec::Text("Puntos: {points}")),
        ],
    );
    let mut l = Localizer::new();
    l.add(en).unwrap();
    l.add(pt).unwrap();
    l.add(es).unwrap();
    l
}

const PLAY: StrKey = StrKey::new("menu.play");
const QUIT: StrKey = StrKey::new("menu.quit");
const GUESTS: StrKey = StrKey::new("guests.count");
const MONEY: StrKey = StrKey::new("money");
const SCORE: StrKey = StrKey::new("score");
const BORN: StrKey = StrKey::new("born");

#[test]
fn tr_follows_the_language_and_its_fallback() {
    let mut l = fixture();
    assert_eq!(l.tag(), "en-US", "the first table is current");
    assert_eq!(l.tr(PLAY), "Play");
    assert!(l.set_language(LangId::new("pt-BR")));
    assert_eq!(l.tr(PLAY), "Jogar");
    assert_eq!(l.tr(QUIT), "Quit", "pt-BR lacks it: en-US's");
    assert_eq!(l.tr(StrKey::new("nowhere")), "");
    assert_eq!(l.get(StrKey::new("nowhere")), None);
    assert!(!l.set_language(LangId::new("ja-JP")), "not loaded");
    assert_eq!(l.tag(), "pt-BR", "a failed switch keeps the language");
    // The UI's view of it.
    assert_eq!(UiStrings::get(&l, "menu.play"), Some("Jogar"));
    let rev = UiStrings::revision(&l);
    l.set_language(LangId::new("es-ES"));
    assert_ne!(
        UiStrings::revision(&l),
        rev,
        "a switch lays the UI out again"
    );
    assert_eq!(
        l.table(LangId::new("pt-BR")).unwrap().name(),
        "Português (Brasil)"
    );
}

#[test]
fn plurals_pick_their_form_per_language() {
    let mut l = fixture();
    let mut buf = TextBuf::<64>::new();
    let n = |v| [Arg::int("n", v)];
    assert_eq!(l.tr_fmt(GUESTS, &n(1), &mut buf), "1 guest");
    assert_eq!(l.tr_fmt(GUESTS, &n(0), &mut buf), "0 guests");
    assert_eq!(l.tr_fmt(GUESTS, &n(1200), &mut buf), "1,200 guests");
    l.set_language(LangId::new("pt-BR"));
    // CLDR pt: one is i = 0..1.
    assert_eq!(l.tr_fmt(GUESTS, &n(0), &mut buf), "0 convidado");
    assert_eq!(l.tr_fmt(GUESTS, &n(1), &mut buf), "1 convidado");
    assert_eq!(l.tr_fmt(GUESTS, &n(2), &mut buf), "2 convidados");
    assert_eq!(l.tr_fmt(GUESTS, &n(1200), &mut buf), "1.200 convidados");
    l.set_language(LangId::new("es-ES"));
    assert_eq!(l.tr_fmt(GUESTS, &n(1), &mut buf), "1 invitado");
    assert_eq!(l.tr_fmt(GUESTS, &n(0), &mut buf), "0 invitados");
    // es groups from five digits (CLDR minimumGroupingDigits = 2).
    assert_eq!(l.tr_fmt(GUESTS, &n(1200), &mut buf), "1200 invitados");
    assert_eq!(l.tr_fmt(GUESTS, &n(12000), &mut buf), "12.000 invitados");
    assert_eq!(l.plural(GUESTS, 1), "{n} invitado");
    // A plural entry read as text is its `other` form.
    assert_eq!(l.tr(GUESTS), "{n} invitados");
}

#[test]
fn money_numbers_and_dates_follow_the_locale() {
    let mut l = fixture();
    let mut buf = TextBuf::<64>::new();
    let cash = [Arg::money("amount", 120_000)];
    assert_eq!(l.tr_fmt(MONEY, &cash, &mut buf), "$1,200.00");
    assert_eq!(
        l.tr_fmt(MONEY, &[Arg::money("amount", -5)], &mut buf),
        "-$0.05"
    );
    assert_eq!(
        l.tr_fmt(BORN, &[Arg::date("d", 2026, 10, 9)], &mut buf),
        "Born 10/09/2026"
    );
    l.set_language(LangId::new("pt-BR"));
    assert_eq!(l.tr_fmt(MONEY, &cash, &mut buf), "R$ 1.200,00");
    assert_eq!(
        l.tr_fmt(BORN, &[Arg::date("d", 2026, 10, 9)], &mut buf),
        "Nasceu em 09/10/2026"
    );
    assert_eq!(
        l.tr_fmt(SCORE, &[Arg::dec("points", 123_456_789, 2)], &mut buf),
        "Pontos: 1.234.567,89"
    );
    l.set_language(LangId::new("es-ES"));
    assert_eq!(l.tr_fmt(MONEY, &cash, &mut buf), "1200,00 €");
    assert_eq!(
        l.tr_fmt(MONEY, &[Arg::money("amount", 1_234_567)], &mut buf),
        "12.345,67 €"
    );
    // A whole-unit integer as currency, a string, a raw year, an unknown
    // argument (written back for QA to see), escapes.
    assert_eq!(
        l.format(
            "{{{x:currency}}} {s} {y:raw} {nope}",
            &[Arg::int("x", 3), Arg::str("s", "olé"), Arg::int("y", 2026)],
            &mut buf
        ),
        "{3,00 €} olé 2026 {nope}"
    );
    // A key the language falls back for is still formatted by the
    // language's own locale.
    assert_eq!(
        l.tr_fmt(BORN, &[Arg::date("d", 2026, 1, 2)], &mut buf),
        "Born 02/01/2026"
    );
}

#[test]
fn a_full_buffer_cuts_the_text_on_a_char_boundary() {
    let mut l = fixture();
    l.set_language(LangId::new("es-ES"));
    let mut buf = TextBuf::<6>::new();
    // "1200,00 €": the euro sign doesn't fit whole.
    let s = l.tr_fmt(MONEY, &[Arg::money("amount", 120_000)], &mut buf);
    assert!(s.len() <= 6 && "1200,00 €".starts_with(s), "{s:?}");
}

#[test]
fn plural_rules_match_cldr() {
    use PluralForm::*;
    let cases: &[(PluralRule, i64, u8, PluralForm)] = &[
        (PluralRule::OneOther, 1, 0, One),
        (PluralRule::OneOther, 1, 1, Other),
        (PluralRule::OneOther, 0, 0, Other),
        (PluralRule::OneUpToOne, 0, 0, One),
        (PluralRule::OneUpToOne, 1, 2, One),
        (PluralRule::OneUpToOne, 2, 0, Other),
        (PluralRule::Other, 1, 0, Other),
        (PluralRule::Slavic, 1, 0, One),
        (PluralRule::Slavic, 21, 0, One),
        (PluralRule::Slavic, 11, 0, Many),
        (PluralRule::Slavic, 3, 0, Few),
        (PluralRule::Slavic, 13, 0, Many),
        (PluralRule::Slavic, 24, 0, Few),
        (PluralRule::Slavic, 5, 0, Many),
        (PluralRule::Slavic, 1, 1, Other),
        (PluralRule::Polish, 1, 0, One),
        (PluralRule::Polish, 21, 0, Many),
        (PluralRule::Polish, 22, 0, Few),
        (PluralRule::Polish, 12, 0, Many),
        (PluralRule::Czech, 3, 0, Few),
        (PluralRule::Czech, 5, 0, Other),
        (PluralRule::Czech, 1, 1, Many),
    ];
    for &(rule, i, v, form) in cases {
        assert_eq!(rule.select(i, v), form, "{rule:?} {i} v={v}");
    }
    assert_eq!(PluralRule::for_tag("pt-BR"), PluralRule::OneUpToOne);
    assert_eq!(PluralRule::for_tag("pt-PT"), PluralRule::OneOther);
    assert_eq!(PluralRule::for_tag("es-ES"), PluralRule::OneOther);
    assert_eq!(PluralRule::for_tag("ru"), PluralRule::Slavic);
    assert_eq!(PluralRule::for_tag("ja-JP"), PluralRule::Other);
    for r in PluralRule::ALL {
        assert_eq!(PluralRule::from_name(r.name()), Some(r));
        assert_eq!(PluralRule::from_u8(r as u8), r);
        assert!(r.forms().contains(&Other));
    }
}

#[test]
fn locale_defaults_by_tag() {
    assert_eq!(Locale::for_tag("en-US"), Locale::EN_US);
    let pt = Locale::for_tag("pt-BR");
    assert_eq!((pt.decimal, pt.thousands, pt.currency), (",", ".", "R$"));
    assert!(pt.currency_space && !pt.currency_after);
    let es = Locale::for_tag("es-ES");
    assert_eq!(
        (es.currency, es.currency_after, es.min_group),
        ("€", true, 2)
    );
    assert_eq!(Locale::for_tag("ja").date_order, DateOrder::Ymd);
    assert_eq!(Locale::for_tag("ja").currency_decimals, 0);
    assert_eq!(Locale::for_tag("de-DE").date_sep, ".");
    assert_eq!(Locale::for_tag("xx").plural, PluralRule::OneOther);
}

#[test]
fn tstr_v2_round_trips_its_locale_fonts_and_group() {
    let ui = FontId::from_path("fonts/ui");
    let ui_pt = FontId::from_path("fonts/ui@pt-BR");
    let fonts = [(ui.0, ui_pt.0)];
    let locale = Locale {
        currency: "R$",
        ..Locale::for_tag("pt-BR")
    };
    let spec = TableSpec {
        lang: "pt-BR",
        fallback: "en-US",
        name: "Português",
        locale,
        fonts: &fonts,
        group: GroupId::from_name("i18n-pt-BR").0,
    };
    let bytes = bake(
        &spec,
        &[("a", EntrySpec::Text("á")), ("b", plural("um", "muitos"))],
    );
    let t = StringTable::parse(bytes).unwrap();
    assert_eq!(t.version(), 2);
    assert_eq!(
        (t.lang(), t.fallback(), t.name()),
        ("pt-BR", "en-US", "Português")
    );
    assert_eq!(t.locale(), locale);
    assert_eq!(t.font(ui), ui_pt);
    assert_eq!(t.font(FontId(7)), FontId(7));
    assert_eq!(t.group(), Some(GroupId::from_name("i18n-pt-BR")));
    match t.entry(StrKey::new("b")).unwrap() {
        Entry::Plural(p) => {
            assert_eq!(p.get(PluralForm::One), "um");
            assert_eq!(p.get(PluralForm::Few), "");
            assert_eq!(p.pick(PluralForm::Few), "muitos");
        }
        e => panic!("{e:?}"),
    }
    // The parts rebuild the same table (what `i18n export --rust` emits).
    let (h, f, r, p) = t.parts();
    let again = StringTable::from_parts(h, f, r, p);
    assert_eq!(again.lookup("a"), Some("á"));
    assert_eq!(again.font(ui), ui_pt);
    // Damage is refused, not read past.
    for cut in [10, 79, bytes.len() - 1] {
        assert!(StringTable::parse(&bytes[..cut]).is_err(), "cut at {cut}");
    }
    let mut v9 = bytes.to_vec();
    v9[4] = 9;
    assert_eq!(
        StringTable::parse(&v9).unwrap_err(),
        StringsError::UnsupportedVersion(9)
    );
}

/// A version 1 table (issue #58's bake) still reads, with its tag's
/// default locale: the migration ADR 0005 §3 asks for.
#[test]
fn tstr_v1_still_parses() {
    // magic, version 1, 1 entry, lang (0, 5), fallback (5, 0), then a record.
    let mut b = Vec::new();
    b.extend_from_slice(b"TSTR");
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    for v in [0u32, 5, 5, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&trino_core::asset_id("k").to_le_bytes());
    b.extend_from_slice(&5u32.to_le_bytes());
    b.extend_from_slice(&5u32.to_le_bytes()); // "Olá!" is 5 bytes
    b.extend_from_slice("pt-BROlá!".as_bytes());
    let t = StringTable::parse(&b).unwrap();
    assert_eq!(t.version(), 1);
    assert_eq!(t.lang(), "pt-BR");
    assert_eq!(t.lookup("k"), Some("Olá!"));
    assert_eq!(t.locale(), Locale::for_tag("pt-BR"));
    assert_eq!(t.name(), "pt-BR");
    assert_eq!(t.group(), None);
}

#[test]
fn the_language_is_saved_then_the_system_then_the_default() {
    let mut l = fixture();
    assert_eq!(
        l.best_match("pt-PT"),
        Some(LangId::new("pt-BR")),
        "same language"
    );
    assert_eq!(l.best_match("ES-es"), Some(LangId::new("es-ES")), "case");
    assert_eq!(l.best_match("ja"), None);
    let sys = LangTag::parse("pt_BR.UTF-8");
    assert_eq!(sys.unwrap().as_str(), "pt-BR");
    assert_eq!(l.pick(None, sys), LangId::new("pt-BR"));
    assert_eq!(
        l.pick(Some(LangId::new("es-ES")), sys),
        LangId::new("es-ES")
    );
    // A saved language the build no longer has: the system's.
    assert_eq!(
        l.pick(Some(LangId::new("de-DE")), sys),
        LangId::new("pt-BR")
    );
    // The 3DS reports a bare language.
    assert_eq!(l.pick(None, LangTag::parse("es")), LangId::new("es-ES"));
    // Nothing fits: the default (first) table.
    assert_eq!(l.pick(None, LangTag::parse("ja")), LangId::new("en-US"));
    assert_eq!(LangTag::parse("C"), None);
    assert_eq!(LangTag::parse("C.UTF-8"), None);
    assert_eq!(LangTag::parse("en_US@euro").unwrap().as_str(), "en-US");
    assert_eq!(LangTag::parse("a-very-long-language-tag"), None);
}

#[test]
fn a_reloaded_table_replaces_its_language() {
    let mut l = fixture();
    let fresh = table(
        "pt-BR",
        "en-US",
        &[("menu.play", EntrySpec::Text("Jogar já"))],
    );
    l.add(fresh).unwrap();
    assert_eq!(l.languages().count(), 3);
    l.set_language(LangId::new("pt-BR"));
    assert_eq!(l.tr(PLAY), "Jogar já");
    let mut small: Localizer<'static, 1> = Localizer::new();
    small.add(fresh).unwrap();
    assert!(small.add(table("es-ES", "", &[])).is_err());
}

#[test]
fn a_fallback_cycle_ends() {
    let a = table("en-US", "pt-BR", &[]);
    let b = table("pt-BR", "en-US", &[]);
    let mut l: Localizer<'static, 2> = Localizer::new();
    l.add(a).unwrap();
    l.add(b).unwrap();
    assert_eq!(l.get(PLAY), None);
}
