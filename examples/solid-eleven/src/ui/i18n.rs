//! Every string the game shows, in English and Brazilian Portuguese (the
//! reference art's language). The texts live in the engine's string tables
//! (issue #59): `assets/i18n/en-US.ron` and `pt-BR.ron`, checked by `cargo
//! xtask i18n check --dir examples/solid-eleven` and exported as static
//! code (`strings_gen.rs`, `cargo xtask i18n export --rust
//! examples/solid-eleven/src/ui/strings_gen.rs --dir examples/solid-eleven`;
//! a test fails on a stale export). [`S`] names each string and [`tr`]
//! reads it. Text is uppercase where the UI font only has capitals.

use trino_core::TextBuf;
use trino_core::i18n::{Arg, LangId, Localizer, StrKey, StringTable};

#[allow(clippy::all)]
mod tables {
    include!("strings_gen.rs");
}

/// Interface language.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Lang {
    #[default]
    En,
    Pt,
}

impl Lang {
    /// The engine's id of the language's table.
    pub const fn id(self) -> LangId {
        match self {
            Lang::En => LangId::new("en-US"),
            Lang::Pt => LangId::new("pt-BR"),
        }
    }

    /// The language's own name (`ENGLISH`, `PORTUGUÊS`: the tables' `name`).
    pub fn name(self) -> &'static str {
        LOC.table(self.id()).map_or("", |t| t.name())
    }
}

/// Both tables: en-US first (the default), pt-BR falling back to it.
pub static LOC: Localizer<'static, 2> =
    Localizer::from_tables([Some(tables::EN_US), Some(tables::PT_BR)]);

/// The baked tables, for tests.
pub const TABLES: [StringTable<'static>; 2] = tables::TABLES;

macro_rules! strings {
    ($($v:ident => $k:literal,)*) => {
        /// A string id; [`tr`] gives its text.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        #[allow(non_camel_case_types)]
        #[repr(u8)]
        pub enum S {
            $($v,)*
        }

        impl S {
            /// The key of `self` in the string tables (hashed at compile
            /// time).
            pub const fn key(self) -> StrKey {
                match self {
                    $(S::$v => {
                        const K: StrKey = StrKey::new($k);
                        K
                    })*
                }
            }

            /// The key as written in the tables.
            pub const fn key_str(self) -> &'static str {
                match self {
                    $(S::$v => $k,)*
                }
            }
        }
    };
}

strings! {
    Goal => "goal",
    OwnGoal => "own_goal",
    Offside => "offside",
    Foul => "foul",
    YellowCard => "yellow_card",
    RedCard => "red_card",
    Corner => "corner",
    ThrowIn => "throw_in",
    GoalKick => "goal_kick",
    FreeKick => "free_kick",
    Penalty => "penalty",
    HalfTime => "half_time",
    FullTime => "full_time",
    KickOff => "kick_off",
    Save => "save",
    Post => "post",
    PlayNow => "play_now",
    QuickMatch => "quick_match",
    Career => "career",
    ManageClub => "manage_club",
    Customize => "customize",
    CustomizeSub => "customize_sub",
    Transfers => "transfers",
    TransfersSub => "transfers_sub",
    Settings => "settings",
    SettingsSub => "settings_sub",
    Exit => "exit",
    ExitSub => "exit_sub",
    Select => "select",
    Back => "back",
    Confirm => "confirm",
    Details => "details",
    Formation => "formation",
    Menu => "menu",
    Squad => "squad",
    Tactics => "tactics",
    TeamSelect => "team_select",
    PlayerEdit => "player_edit",
    PointsLeft => "points_left",
    Save_ => "save_action",
    Cancel => "cancel",
    Random => "random",
    Buy => "buy",
    Sell => "sell",
    Balance => "balance",
    Coins => "coins",
    Price => "price",
    Position => "position",
    Ovr => "ovr",
    Team => "team",
    Age => "age",
    Club => "club",
    Finances => "finances",
    Players => "players",
    Matches => "matches",
    NextMatch => "next_match",
    Round => "round",
    Play => "play",
    Simulate => "simulate",
    WeeklyIncome => "weekly_income",
    Wages => "wages",
    Positive => "positive",
    Negative => "negative",
    Morale => "morale",
    Energy => "energy",
    Fitness => "fitness",
    LastResults => "last_results",
    Home => "home",
    Away => "away",
    Stadium => "stadium",
    Weather => "weather",
    Clear => "clear",
    Rain => "rain",
    Day => "day",
    Dusk => "dusk",
    Night => "night",
    Length => "length",
    Minutes => "minutes",
    Difficulty => "difficulty",
    Easy => "easy",
    Normal => "normal",
    Hard => "hard",
    Language => "language",
    Camera => "camera",
    Broadcast => "broadcast",
    Close => "close",
    Results => "results",
    Possession => "possession",
    Shots => "shots",
    OnTarget => "on_target",
    Passes => "passes",
    Fouls => "fouls",
    Corners => "corners",
    Offsides => "offsides",
    Continue => "continue",
    Paused => "paused",
    Resume => "resume",
    Quit => "quit",
    Speed => "speed",
    Dribble => "dribble",
    Shot => "shot",
    Pass => "pass",
    Stamina => "stamina",
    Defense => "defense",
    Table => "table",
    Pts => "pts",
    Champion => "champion",
    ForSale => "for_sale",
    NotEnough => "not_enough",
    SquadFull => "squad_full",
    SquadMin => "squad_min",
    Bought => "bought",
    Sold => "sold",
    Filter => "filter",
    Compare => "compare",
    Win => "win",
    Draw => "draw",
    Loss => "loss",
    Controls => "controls",
    Credits => "credits",
    PressStart => "press_start",
    Fame => "fame",
    Level => "level",
    Saved => "saved",
    Swap => "swap",
    Market => "market",
    Substitution => "substitution",
    Substitute => "substitute",
    LeagueWon => "league_won",
    FinishedAt => "finished_at",
    Season => "season",
    Keeper => "keeper",
    KeeperAi => "keeper_ai",
    KeeperYou => "keeper_you",
    Graphics => "graphics",
    GfxMax => "gfx_max",
    GfxRetro => "gfx_retro",
    ControlsSub => "controls_sub",
    TabMatch => "tab_match",
    TabKeyboard => "tab_keyboard",
    TabPad => "tab_pad",
    ActPass => "act_pass",
    ActShoot => "act_shoot",
    ActThrough => "act_through",
    ActKeeper => "act_keeper",
    ActSwitch => "act_switch",
    ActSprint => "act_sprint",
    ActPause => "act_pause",
    ShortPass => "short_pass",
    ShortShoot => "short_shoot",
    ShortThrough => "short_through",
    ShortKeeper => "short_keeper",
    ShortSwitch => "short_switch",
    ShortSprint => "short_sprint",
    ShortPause => "short_pause",
    ResetDefaults => "reset_defaults",
    PressButton => "press_button",
    PressKey => "press_key",
    PressPad => "press_pad",
    NoPad => "no_pad",
    PadUnsupported => "pad_unsupported",
    MoveStick => "move_stick",
    MoveKeys => "move_keys",
    NotOnPad => "not_on_pad",
    TabsHint => "tabs_hint",
    Change => "change",
    OtherKeys => "other_keys",
    Action => "action",
    Button => "button",
    Hair => "hair",
    HairCrop => "hair_crop",
    HairBuzz => "hair_buzz",
    HairCurly => "hair_curly",
    HairLong => "hair_long",
    HairMohawk => "hair_mohawk",
    HairBald => "hair_bald",
    KickoffTime => "kickoff_time",
    StadiumArena => "stadium_arena",
    StadiumHarbour => "stadium_harbour",
    StadiumPark => "stadium_park",
    Bench => "bench",
    OnPitch => "on_pitch",
    SubsRemaining => "subs_remaining",
    NoSubsLeft => "no_subs_left",
    SideHome => "side_home",
    SideAway => "side_away",
    SideCpu => "side_cpu",
}

impl S {
    /// The last string id: tests walk the enum up to it, so a string added
    /// after it must move it.
    pub const LAST: S = S::SideCpu;

    /// A hair style's name, by [`crate::render::HairStyle`] index.
    pub fn hair_style(i: usize) -> S {
        [
            S::HairCrop,
            S::HairBuzz,
            S::HairCurly,
            S::HairLong,
            S::HairMohawk,
            S::HairBald,
        ][i % 6]
    }
}

/// The text of `s` in `lang` (pt-BR falls back to en-US).
pub fn tr(lang: Lang, s: S) -> &'static str {
    LOC.get_in(lang.id(), s.key()).unwrap_or("")
}

/// `s` with its placeholders filled by `lang`'s locale.
pub fn tr_fmt<'b, const N: usize>(
    lang: Lang,
    s: S,
    args: &[Arg<'_>],
    out: &'b mut TextBuf<N>,
) -> &'b str {
    LOC.tr_fmt_in(lang.id(), s.key(), args, out)
}

/// Position abbreviations as in lists (GK/DF/MF/FW; GOL/ZAG/MEI/ATA).
pub fn pos_short(lang: Lang, p: crate::sim::team::Pos) -> &'static str {
    use crate::sim::team::Pos;
    const GK: StrKey = StrKey::new("pos.gk");
    const DF: StrKey = StrKey::new("pos.df");
    const MF: StrKey = StrKey::new("pos.mf");
    const FW: StrKey = StrKey::new("pos.fw");
    let key = match p {
        Pos::Gk => GK,
        Pos::Df => DF,
        Pos::Mf => MF,
        Pos::Fw => FW,
    };
    LOC.get_in(lang.id(), key).unwrap_or("")
}

/// The UI system's string keys (`ui/*.ui.ron`, issue #58) read through
/// [`tr`]: the screens stay in the game's own two languages.
pub struct UiTr(pub Lang);

impl trino_core::ui::UiStrings for UiTr {
    fn get(&self, key: &str) -> Option<&str> {
        let s = match key {
            "pause.title" => S::Paused,
            "pause.resume" => S::Resume,
            "pause.substitute" => S::Substitute,
            "pause.quit" => S::Quit,
            _ => return None,
        };
        Some(tr(self.0, s))
    }

    fn revision(&self) -> u32 {
        self.0 as u32 + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> impl Iterator<Item = S> {
        // Walk the enum through its discriminants.
        (0..=S::LAST as u8).map(|i| unsafe { core::mem::transmute::<u8, S>(i) })
    }

    /// Every string is in both languages' own tables (no fallback) and,
    /// formatted, uses only characters the UI font bakes.
    #[test]
    fn every_string_fits_the_font() {
        const CHARS: &str = super::super::FONT_CHARS;
        let mut n = 0;
        let mut buf = TextBuf::<64>::new();
        for s in all() {
            for (lang, table) in [(Lang::En, TABLES[0]), (Lang::Pt, TABLES[1])] {
                assert!(
                    table.entry(s.key()).is_some(),
                    "{s:?}: \"{}\" is not in {}",
                    s.key_str(),
                    table.lang()
                );
                let t = tr_fmt(lang, s, &[Arg::int("n", 90)], &mut buf);
                assert!(!t.is_empty());
                for c in t.chars() {
                    assert!(
                        CHARS.contains(c),
                        "{s:?}/{lang:?}: '{c}' is not in the font"
                    );
                }
            }
            n += 1;
        }
        assert_eq!(n, S::LAST as usize + 1);
        assert_eq!(Lang::En.name(), "ENGLISH");
        assert_eq!(Lang::Pt.name(), "PORTUGUÊS");
    }

    /// The strings the code used to hold, through the tables.
    #[test]
    fn the_tables_say_what_the_code_said() {
        use crate::sim::team::Pos;
        assert_eq!(tr(Lang::En, S::Goal), "GOAL!");
        assert_eq!(tr(Lang::Pt, S::YellowCard), "CARTÃO AMARELO");
        assert_eq!(tr(Lang::Pt, S::SideCpu), "CPU X CPU");
        assert_eq!(pos_short(Lang::Pt, Pos::Df), "ZAG");
        let mut buf = TextBuf::<16>::new();
        assert_eq!(
            tr_fmt(Lang::Pt, S::Minutes, &[Arg::int("n", 5)], &mut buf),
            "5 MIN"
        );
    }
}
