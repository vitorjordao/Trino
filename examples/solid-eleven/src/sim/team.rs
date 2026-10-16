//! Teams, players, attributes and formations: the data the match and the
//! career share. Ten fictional clubs, each with a kit, a badge and a
//! tactical identity; 16 players each, generated deterministically from the
//! club's identity and balanced by a **point budget** (every club gets the
//! same total, so differences are style, not strength).

use super::fx::{Fx, V2};

/// Players per squad (11 starters + 5 on the bench) at the start.
pub const SQUAD: usize = 16;
/// Clubs in the league.
pub const TEAMS: usize = 10;
/// Players in the database.
pub const PLAYERS: usize = TEAMS * SQUAD;
/// A squad may grow to this through transfers.
pub const MAX_SQUAD: usize = 20;
/// Smallest squad a sale may leave.
pub const MIN_SQUAD: usize = 13;

/// Attribute indices.
pub const SPEED: usize = 0;
pub const DRIBBLE: usize = 1;
pub const SHOT: usize = 2;
pub const PASS: usize = 3;
pub const STAMINA: usize = 4;
pub const DEFENSE: usize = 5;
pub const ATTRS: usize = 6;

/// Lowest and highest value of any attribute.
pub const ATTR_MIN: u8 = 30;
pub const ATTR_MAX: u8 = 99;

/// Point budgets: the sum of a player's six attributes may not exceed his
/// budget (the cap that keeps edited players from becoming all-99s).
pub const BUDGET_STAR: u16 = 462;
pub const BUDGET_REGULAR: u16 = 426;
pub const BUDGET_BENCH: u16 = 396;

/// Field positions as shown in lists.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Pos {
    Gk,
    Df,
    Mf,
    Fw,
}

/// What a formation slot asks of its player (drives the AI).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    Gk,
    /// Center back.
    Cb,
    /// Full back: overlaps on the wing.
    Fb,
    /// Holding midfielder.
    Dm,
    Cm,
    /// Attacking midfielder.
    Am,
    /// Winger: holds the width, takes on full backs.
    W,
    /// Striker: runs behind the line.
    St,
}

impl Role {
    pub fn pos(self) -> Pos {
        match self {
            Role::Gk => Pos::Gk,
            Role::Cb | Role::Fb => Pos::Df,
            Role::Dm | Role::Cm | Role::Am => Pos::Mf,
            Role::W | Role::St => Pos::Fw,
        }
    }

    /// Attribute weights for the overall rating (sum 20) and the template
    /// the generator starts from.
    fn weights(self) -> [u8; ATTRS] {
        match self {
            Role::Gk => [1, 0, 0, 3, 2, 14],
            Role::Cb => [3, 1, 0, 3, 3, 10],
            Role::Fb => [5, 2, 1, 4, 4, 4],
            Role::Dm => [2, 2, 1, 6, 4, 5],
            Role::Cm => [3, 4, 2, 7, 3, 1],
            Role::Am => [3, 6, 5, 5, 1, 0],
            Role::W => [7, 6, 4, 2, 1, 0],
            Role::St => [5, 3, 10, 1, 1, 0],
        }
    }

    fn template(self) -> [u8; ATTRS] {
        match self {
            Role::Gk => [48, 35, 34, 58, 62, 92],
            Role::Cb => [62, 46, 40, 62, 72, 88],
            Role::Fb => [80, 62, 46, 68, 82, 74],
            Role::Dm => [64, 58, 52, 78, 84, 78],
            Role::Cm => [68, 72, 62, 84, 80, 60],
            Role::Am => [74, 86, 78, 82, 68, 44],
            Role::W => [90, 84, 70, 72, 74, 40],
            Role::St => [82, 76, 90, 64, 70, 36],
        }
    }
}

/// A formation: 11 slots (GK first) with a base position and a role.
/// Positions are in **team space**, normalized: `x` from -1 (own goal
/// line) to +1 (the opponents'), `y` from -1 to 1 across; the shape with
/// the ball at the center spot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Formation {
    pub name: &'static str,
    pub slots: [(i16, i16, Role); 11],
}

impl Formation {
    /// Slot `i`'s base position (normalized, in thousandths).
    pub fn base(&self, i: usize) -> V2 {
        let (x, y, _) = self.slots[i];
        V2::new(Fx::milli(x as i32), Fx::milli(y as i32))
    }

    pub fn role(&self, i: usize) -> Role {
        self.slots[i].2
    }
}

use Role::*;

pub const F442: Formation = Formation {
    name: "4-4-2",
    slots: [
        (-950, 0, Gk),
        (-640, -640, Fb),
        (-690, -220, Cb),
        (-690, 220, Cb),
        (-640, 640, Fb),
        (-300, -660, W),
        (-340, -200, Cm),
        (-340, 200, Dm),
        (-300, 660, W),
        (-60, -170, St),
        (-80, 170, St),
    ],
};

pub const F433: Formation = Formation {
    name: "4-3-3",
    slots: [
        (-950, 0, Gk),
        (-640, -640, Fb),
        (-690, -220, Cb),
        (-690, 220, Cb),
        (-640, 640, Fb),
        (-420, 0, Dm),
        (-300, -360, Cm),
        (-300, 360, Am),
        (-110, -640, W),
        (-60, 0, St),
        (-110, 640, W),
    ],
};

pub const F4231: Formation = Formation {
    name: "4-2-3-1",
    slots: [
        (-950, 0, Gk),
        (-640, -640, Fb),
        (-690, -220, Cb),
        (-690, 220, Cb),
        (-640, 640, Fb),
        (-440, -200, Dm),
        (-440, 200, Cm),
        (-210, -620, W),
        (-200, 0, Am),
        (-210, 620, W),
        (-60, 0, St),
    ],
};

pub const F352: Formation = Formation {
    name: "3-5-2",
    slots: [
        (-950, 0, Gk),
        (-690, -380, Cb),
        (-720, 0, Cb),
        (-690, 380, Cb),
        (-360, -760, Fb),
        (-430, 0, Dm),
        (-320, -300, Cm),
        (-320, 300, Am),
        (-360, 760, Fb),
        (-60, -180, St),
        (-80, 180, St),
    ],
};

pub const F532: Formation = Formation {
    name: "5-3-2",
    slots: [
        (-950, 0, Gk),
        (-600, -720, Fb),
        (-700, -340, Cb),
        (-720, 0, Cb),
        (-700, 340, Cb),
        (-600, 720, Fb),
        (-380, -340, Cm),
        (-420, 0, Dm),
        (-380, 340, Cm),
        (-60, -180, St),
        (-80, 180, St),
    ],
};

pub const FORMATIONS: [&Formation; 5] = [&F442, &F433, &F4231, &F352, &F532];

/// How a club plays (0..100 each).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tactics {
    pub formation: u8,
    /// How far up the pitch they hunt the ball.
    pub press: u8,
    /// Height of the defensive line (and the offside trap).
    pub line: u8,
    /// How wide the shape is in possession.
    pub width: u8,
    /// Risk in passing: short and safe (0) to forward and fast (100).
    pub tempo: u8,
    /// Long balls over the top instead of build-up.
    pub direct: u8,
    /// Hard tackling: more won balls, more fouls.
    pub aggression: u8,
}

impl Tactics {
    pub fn formation(&self) -> &'static Formation {
        FORMATIONS[self.formation as usize % FORMATIONS.len()]
    }
}

/// An sRGB color (the UI and the kits).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Squared distance (weighted for perception), for kit clashes.
    pub fn dist2(self, o: Rgb) -> u32 {
        let d = |a: u8, b: u8| (a as i32 - b as i32).unsigned_abs();
        let (r, g, b) = (d(self.0, o.0), d(self.1, o.1), d(self.2, o.2));
        2 * r * r + 4 * g * g + 3 * b * b
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Kit {
    pub shirt: Rgb,
    /// Collar, cuffs and number color.
    pub trim: Rgb,
    pub shorts: Rgb,
    pub socks: Rgb,
}

/// Badge outline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Crest {
    Shield,
    Round,
    Diamond,
}

/// A club as shipped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Club {
    pub name: &'static str,
    /// Three letters for the scoreboard.
    pub short: &'static str,
    pub home: Kit,
    pub away: Kit,
    pub gk: Rgb,
    pub crest: Crest,
    pub tactics: Tactics,
    /// Attribute bias of the club's identity, added before balancing.
    pub style: [i8; ATTRS],
    /// Fame (stadium level and ticket income).
    pub fame: u8,
}

const fn kit(shirt: Rgb, trim: Rgb, shorts: Rgb, socks: Rgb) -> Kit {
    Kit {
        shirt,
        trim,
        shorts,
        socks,
    }
}

const WHITE: Rgb = Rgb(240, 240, 240);
const NAVY: Rgb = Rgb(28, 36, 72);
const BLACK: Rgb = Rgb(30, 30, 34);

const fn tac(
    formation: u8,
    press: u8,
    line: u8,
    width: u8,
    tempo: u8,
    direct: u8,
    aggression: u8,
) -> Tactics {
    Tactics {
        formation,
        press,
        line,
        width,
        tempo,
        direct,
        aggression,
    }
}

/// The ten clubs. Formation indices refer to [`FORMATIONS`].
pub const CLUBS: [Club; TEAMS] = [
    Club {
        name: "FC Rouge",
        short: "ROU",
        home: kit(Rgb(206, 32, 41), WHITE, WHITE, Rgb(206, 32, 41)),
        away: kit(WHITE, Rgb(206, 32, 41), Rgb(206, 32, 41), WHITE),
        gk: Rgb(60, 190, 80),
        crest: Crest::Shield,
        // Possession and a high press.
        tactics: tac(1, 78, 66, 60, 40, 15, 55),
        style: [0, 4, 0, 6, -4, -6],
        fame: 12,
    },
    Club {
        name: "Azure United",
        short: "AZU",
        home: kit(Rgb(28, 90, 200), WHITE, Rgb(24, 40, 120), Rgb(28, 90, 200)),
        away: kit(
            Rgb(250, 210, 60),
            Rgb(28, 90, 200),
            Rgb(28, 90, 200),
            Rgb(250, 210, 60),
        ),
        gk: Rgb(240, 120, 30),
        crest: Crest::Shield,
        // Balanced, wing play.
        tactics: tac(0, 55, 50, 80, 55, 30, 50),
        style: [4, 2, 0, 0, 0, -6],
        fame: 11,
    },
    Club {
        name: "Verde Rovers",
        short: "VER",
        home: kit(Rgb(20, 150, 70), WHITE, Rgb(20, 150, 70), Rgb(20, 150, 70)),
        away: kit(BLACK, Rgb(20, 150, 70), BLACK, BLACK),
        gk: Rgb(240, 200, 40),
        crest: Crest::Round,
        // Counter-attack: sit, then go direct.
        tactics: tac(2, 35, 38, 55, 75, 60, 55),
        style: [6, 0, 4, -4, 0, -6],
        fame: 9,
    },
    Club {
        name: "Solar City",
        short: "SOL",
        home: kit(Rgb(250, 200, 30), BLACK, BLACK, Rgb(250, 200, 30)),
        away: kit(BLACK, Rgb(250, 200, 30), Rgb(250, 200, 30), BLACK),
        gk: Rgb(200, 60, 200),
        crest: Crest::Round,
        // Pace and a high line.
        tactics: tac(1, 65, 78, 70, 70, 45, 45),
        style: [10, 2, 2, -4, -2, -8],
        fame: 10,
    },
    Club {
        name: "Ivory Athletic",
        short: "IVO",
        home: kit(WHITE, NAVY, NAVY, WHITE),
        away: kit(NAVY, WHITE, WHITE, NAVY),
        gk: Rgb(230, 70, 70),
        crest: Crest::Shield,
        // Deep block.
        tactics: tac(0, 30, 28, 45, 45, 35, 50),
        style: [-4, -2, 0, 2, 2, 2],
        fame: 10,
    },
    Club {
        name: "Obsidian FC",
        short: "OBS",
        home: kit(BLACK, Rgb(240, 120, 20), BLACK, Rgb(240, 120, 20)),
        away: kit(Rgb(240, 120, 20), BLACK, Rgb(240, 120, 20), BLACK),
        gk: Rgb(80, 200, 230),
        crest: Crest::Diamond,
        // Physical, long balls.
        tactics: tac(3, 50, 45, 60, 65, 80, 80),
        style: [-2, -6, 2, -2, 6, 2],
        fame: 8,
    },
    Club {
        name: "Violet Wanderers",
        short: "VIO",
        home: kit(Rgb(120, 50, 170), WHITE, Rgb(120, 50, 170), WHITE),
        away: kit(
            Rgb(220, 220, 225),
            Rgb(120, 50, 170),
            Rgb(120, 50, 170),
            Rgb(120, 50, 170),
        ),
        gk: Rgb(250, 230, 40),
        crest: Crest::Diamond,
        // Technical dribblers.
        tactics: tac(2, 60, 55, 55, 35, 10, 40),
        style: [0, 10, 0, 4, -6, -8],
        fame: 9,
    },
    Club {
        name: "Coral Bay",
        short: "COR",
        home: kit(
            Rgb(250, 110, 70),
            Rgb(20, 150, 150),
            Rgb(20, 150, 150),
            Rgb(250, 110, 70),
        ),
        away: kit(
            Rgb(20, 150, 150),
            Rgb(250, 110, 70),
            WHITE,
            Rgb(20, 150, 150),
        ),
        gk: Rgb(40, 40, 160),
        crest: Crest::Round,
        // All-out attack on the wings.
        tactics: tac(1, 60, 60, 90, 65, 35, 45),
        style: [6, 4, 4, -2, -4, -8],
        fame: 8,
    },
    Club {
        name: "Frost Harbor",
        short: "FRO",
        home: kit(Rgb(120, 190, 240), WHITE, WHITE, Rgb(120, 190, 240)),
        away: kit(NAVY, Rgb(120, 190, 240), NAVY, NAVY),
        gk: Rgb(240, 160, 30),
        crest: Crest::Shield,
        // Five at the back, quick breaks.
        tactics: tac(4, 25, 30, 50, 60, 65, 45),
        style: [2, -2, 2, 0, 0, 4],
        fame: 9,
    },
    Club {
        name: "Crimson Hawks",
        short: "CRI",
        home: kit(
            Rgb(130, 24, 48),
            Rgb(230, 180, 60),
            Rgb(130, 24, 48),
            Rgb(230, 180, 60),
        ),
        away: kit(
            Rgb(230, 180, 60),
            Rgb(130, 24, 48),
            Rgb(230, 180, 60),
            Rgb(130, 24, 48),
        ),
        gk: Rgb(60, 60, 60),
        crest: Crest::Diamond,
        // Aggressive pressing.
        tactics: tac(0, 85, 62, 60, 60, 40, 85),
        style: [2, 0, 0, 0, 6, 4],
        fame: 10,
    },
];

/// Surnames (fictional players: common international family names).
pub const SURNAMES: [&str; 96] = [
    "Silva", "Souza", "Alves", "Lima", "Gomes", "Rocha", "Martinez", "Rossi", "Kim", "Johnson",
    "Costa", "Pereira", "Ferreira", "Santos", "Oliveira", "Ribeiro", "Moreau", "Dubois", "Lambert",
    "Fontaine", "Garcia", "Lopez", "Romero", "Navarro", "Ortega", "Herrera", "Bianchi", "Ricci",
    "Greco", "Conti", "Moretti", "Ferrari", "Muller", "Weber", "Becker", "Hoffmann", "Schulz",
    "Wagner", "Jansen", "Visser", "Bakker", "Smit", "Mulder", "Novak", "Horvat", "Kovac", "Petrov",
    "Ivanov", "Sokolov", "Nowak", "Kowalski", "Lewicki", "Nielsen", "Larsen", "Hansen", "Berg",
    "Lund", "Dahl", "Okafor", "Mensah", "Diallo", "Traore", "Keita", "Toure", "Adeyemi", "Owusu",
    "Tanaka", "Sato", "Suzuki", "Park", "Lee", "Chen", "Wang", "Nguyen", "Tran", "Murphy", "Kelly",
    "Walsh", "Byrne", "Evans", "Hughes", "Price", "Morgan", "Turner", "Baker", "Carter", "Mendes",
    "Teixeira", "Barros", "Cardoso", "Pinto", "Moura", "Duarte", "Vieira", "Campos", "Aguilar",
];

/// One player in the database (mutable: editing, form, transfers).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayerRec {
    pub surname: u8,
    pub number: u8,
    pub pos: Pos,
    pub attrs: [u8; ATTRS],
    /// The cap on the sum of `attrs`.
    pub budget: u16,
    pub age: u8,
    /// Skin tone index (0..8) and hair color index (0..6).
    pub skin: u8,
    pub hair: u8,
    /// Between matches: energy 0..100 and form -5..5.
    pub energy: u8,
    pub form: i8,
}

impl PlayerRec {
    pub fn name(&self) -> &'static str {
        SURNAMES[self.surname as usize % SURNAMES.len()]
    }

    pub fn total(&self) -> u16 {
        self.attrs.iter().map(|&a| a as u16).sum()
    }

    /// Points left under the budget.
    pub fn available(&self) -> i32 {
        self.budget as i32 - self.total() as i32
    }

    /// Overall rating for the player's listed position.
    pub fn ovr(&self) -> u8 {
        ovr_for(&self.attrs, pos_role(self.pos))
    }

    /// Raise or lower one attribute by `delta`, keeping it in
    /// [`ATTR_MIN`]..=[`ATTR_MAX`] and the sum within the budget.
    pub fn adjust(&mut self, attr: usize, delta: i8) -> Result<(), CapError> {
        let cur = self.attrs[attr] as i32;
        let new = cur + delta as i32;
        if new < ATTR_MIN as i32 {
            return Err(CapError::Floor);
        }
        if new > ATTR_MAX as i32 {
            return Err(CapError::Ceiling);
        }
        if delta > 0 && self.available() < delta as i32 {
            return Err(CapError::NoPoints);
        }
        self.attrs[attr] = new as u8;
        Ok(())
    }

    /// Market value in coins: quadratic in the overall rating above 65
    /// (an 85 is worth about 8 700, a 78 about 3 800), lower for veterans,
    /// higher for prospects; rounded to 100.
    pub fn value(&self) -> u32 {
        let d = (self.ovr() as i32 * 2 - 131).max(0) as u32;
        let mut v = d * d * 2235 / 400 + 300;
        if self.age > 30 {
            v = v * (40 - self.age.min(38) as u32) / 10;
        } else if self.age < 23 {
            v = v * 12 / 10;
        }
        (v / 100).max(3) * 100
    }

    /// Wage per round.
    pub fn wage(&self) -> u32 {
        (self.value() / 60 / 10).max(3) * 10
    }
}

/// Why an attribute edit was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CapError {
    Floor,
    Ceiling,
    /// The point budget is spent: lower another attribute first.
    NoPoints,
}

/// The overall rating `attrs` give in `role` (weighted mean).
pub fn ovr_for(attrs: &[u8; ATTRS], role: Role) -> u8 {
    let w = role.weights();
    let mut s = 0u32;
    let mut n = 0u32;
    for i in 0..ATTRS {
        s += attrs[i] as u32 * w[i] as u32;
        n += w[i] as u32;
    }
    ((s + n / 2) / n) as u8
}

/// The role used to rate a listed position.
pub fn pos_role(p: Pos) -> Role {
    match p {
        Pos::Gk => Role::Gk,
        Pos::Df => Role::Cb,
        Pos::Mf => Role::Cm,
        Pos::Fw => Role::St,
    }
}

/// Bring `attrs` to exactly `budget` points, spreading the change over the
/// attributes in turn and keeping each within bounds.
pub fn balance(attrs: &mut [u8; ATTRS], budget: u16) {
    let mut guard = 0;
    loop {
        let total: i32 = attrs.iter().map(|&a| a as i32).sum();
        let diff = budget as i32 - total;
        if diff == 0 || guard > 2000 {
            return;
        }
        let i = guard % ATTRS;
        guard += 1;
        if diff > 0 && attrs[i] < ATTR_MAX {
            attrs[i] += 1;
        } else if diff < 0 && attrs[i] > ATTR_MIN {
            attrs[i] -= 1;
        }
    }
}

/// A small deterministic hash for generation (no RNG state to thread).
const fn mix(a: u32, b: u32) -> u32 {
    let mut x = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA77);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x
}

/// Squad slot `s` (0..16) of a club: the formation's 11 then a bench of
/// GK, DF, MF, MF, FW.
fn squad_role(club: &Club, s: usize) -> Role {
    if s < 11 {
        return club.tactics.formation().role(s);
    }
    [Role::Gk, Role::Cb, Role::Cm, Role::W, Role::St][s - 11]
}

/// Generate player `s` of club `c`.
pub fn generate_player(c: usize, s: usize) -> PlayerRec {
    let club = &CLUBS[c];
    let role = squad_role(club, s);
    let h = mix(c as u32 + 1, s as u32 + 17);
    let mut attrs = role.template();
    for (i, a) in attrs.iter_mut().enumerate() {
        let jitter = (mix(h, i as u32) % 9) as i32 - 4;
        let v = *a as i32 + club.style[i] as i32 + jitter;
        *a = v.clamp(ATTR_MIN as i32, ATTR_MAX as i32) as u8;
    }
    // Two stars per club (a striker or winger and a midfielder or
    // defender, by formation), nine regulars, five on the bench.
    let star = s == 9 || s == 6;
    let budget = if s >= 11 {
        BUDGET_BENCH
    } else if star {
        BUDGET_STAR
    } else {
        BUDGET_REGULAR
    };
    balance(&mut attrs, budget);
    // Numbers: GK 1, then the usual back-to-front order; bench 12+.
    let number = if s < 11 {
        [1, 2, 4, 5, 3, 6, 8, 10, 7, 9, 11][s]
    } else {
        s as u8 + 1
    };
    PlayerRec {
        surname: ((c * 37 + s * 11 + (h as usize % 7)) % SURNAMES.len()) as u8,
        number,
        pos: role.pos(),
        attrs,
        budget,
        age: 18 + (mix(h, 99) % 17) as u8,
        skin: (mix(h, 7) % 8) as u8,
        hair: (mix(h, 8) % 6) as u8,
        energy: 100,
        form: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clubs_are_balanced_by_points() {
        let mut totals = [0u32; TEAMS];
        for (c, t) in totals.iter_mut().enumerate() {
            for s in 0..SQUAD {
                let p = generate_player(c, s);
                assert_eq!(p.total(), p.budget, "club {c} player {s}");
                assert!(p.attrs.iter().all(|&a| (ATTR_MIN..=ATTR_MAX).contains(&a)));
                *t += p.total() as u32;
            }
        }
        assert!(totals.iter().all(|&t| t == totals[0]), "{totals:?}");
    }

    #[test]
    fn identities_show_in_the_attributes() {
        // Solar City is the pace club, Violet Wanderers the dribblers.
        let mean = |c: usize, a: usize| -> u32 {
            (0..11)
                .map(|s| generate_player(c, s).attrs[a] as u32)
                .sum::<u32>()
                / 11
        };
        assert!(mean(3, SPEED) > mean(4, SPEED) + 4);
        assert!(mean(6, DRIBBLE) > mean(5, DRIBBLE) + 6);
    }

    #[test]
    fn every_squad_has_keepers_and_a_full_eleven() {
        for c in 0..TEAMS {
            let gks = (0..SQUAD)
                .filter(|&s| generate_player(c, s).pos == Pos::Gk)
                .count();
            assert_eq!(gks, 2, "club {c}");
            assert_eq!(generate_player(c, 0).pos, Pos::Gk);
            let f = CLUBS[c].tactics.formation();
            assert_eq!(f.role(0), Role::Gk);
            assert_eq!((1..11).filter(|&i| f.role(i) == Role::Gk).count(), 0);
            // Every base position is inside its own half at kick-off.
            for i in 0..11 {
                let b = f.base(i);
                assert!(b.x <= Fx::ZERO && b.x >= -Fx::ONE, "{} slot {i}", f.name);
                assert!(b.y.abs() <= Fx::ONE);
            }
        }
    }

    #[test]
    fn the_point_cap_holds_while_editing() {
        let mut p = generate_player(0, 9);
        assert_eq!(p.available(), 0);
        assert_eq!(p.adjust(SPEED, 1), Err(CapError::NoPoints));
        // Lowering one attribute frees points for another.
        p.adjust(DEFENSE, -5).unwrap();
        assert_eq!(p.available(), 5);
        let before = p.attrs[SHOT];
        for _ in 0..5 {
            p.adjust(SHOT, 1).unwrap_or(());
        }
        assert!(p.attrs[SHOT] <= ATTR_MAX);
        assert!(p.total() <= p.budget);
        assert!(p.attrs[SHOT] > before || before == ATTR_MAX);
        // Bounds.
        p.attrs[PASS] = ATTR_MIN;
        assert_eq!(p.adjust(PASS, -1), Err(CapError::Floor));
        p.attrs[PASS] = ATTR_MAX;
        p.budget = 999;
        assert_eq!(p.adjust(PASS, 1), Err(CapError::Ceiling));
    }

    #[test]
    fn value_rises_with_rating() {
        let mut p = generate_player(1, 9);
        p.age = 26;
        p.attrs = [75; ATTRS];
        let v = p.value();
        p.attrs = [99; ATTRS];
        assert!(p.value() > v * 2, "{v} -> {}", p.value());
        assert_eq!(p.value() % 100, 0);
        assert!(p.wage() > 0);
    }

    #[test]
    fn kits_of_any_two_clubs_can_be_told_apart() {
        // Home against home, or home against the away kit: one pair must
        // be far enough apart (the match picks it).
        for a in 0..TEAMS {
            for b in 0..TEAMS {
                if a == b {
                    continue;
                }
                let h = CLUBS[a].home.shirt;
                let best = CLUBS[b]
                    .home
                    .shirt
                    .dist2(h)
                    .max(CLUBS[b].away.shirt.dist2(h));
                assert!(best > 30_000, "{} vs {}", CLUBS[a].name, CLUBS[b].name);
            }
        }
    }
}
