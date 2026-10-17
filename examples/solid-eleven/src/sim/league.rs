//! The career: ten clubs, a season of single round-robin rounds, a league
//! table, the clubs' money, form/morale/energy between matches and the
//! transfer market. Pure and deterministic like the match; the save is a
//! compact byte layout that fits the N64 cartridge's 2 KB EEPROM.

use super::engine::{BENCH, Setup, SideSetup};
use super::fx::Rng;
use super::team::{
    self, ATTRS, CLUBS, FORMATIONS, MAX_SQUAD, MIN_SQUAD, PLAYERS, PlayerRec, Pos, Role, SQUAD,
    TEAMS, Tactics, ovr_for,
};

/// Rounds in a season (every club meets every other once).
pub const ROUNDS: usize = TEAMS - 1;
/// Matches per round.
pub const PER_ROUND: usize = TEAMS / 2;
/// No club in this player slot.
pub const NO_CLUB: u8 = 255;
/// Coins every club starts a career with (the reference art's balance).
pub const START_BUDGET: i32 = 15_200;

/// A table row.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Row {
    pub played: u8,
    pub won: u8,
    pub drawn: u8,
    pub lost: u8,
    pub gf: u8,
    pub ga: u8,
}

impl Row {
    pub fn points(&self) -> u8 {
        self.won * 3 + self.drawn
    }

    pub fn gd(&self) -> i16 {
        self.gf as i16 - self.ga as i16
    }

    fn add(&mut self, gf: u8, ga: u8) {
        self.played += 1;
        self.gf = self.gf.saturating_add(gf);
        self.ga = self.ga.saturating_add(ga);
        if gf > ga {
            self.won += 1;
        } else if gf == ga {
            self.drawn += 1;
        } else {
            self.lost += 1;
        }
    }
}

/// A club between matches.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ClubState {
    pub budget: i32,
    /// 0..100, 50 neutral.
    pub morale: u8,
    pub formation: u8,
}

/// Why a transfer was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TransferError {
    NotEnoughCoins,
    SquadFull,
    SquadTooSmall,
    NoBuyer,
    OwnPlayer,
}

/// Results of the user's last matches (newest last): 0 = none, 1 W, 2 D,
/// 3 L.
pub type Form = [u8; 5];

/// The whole career state.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Career {
    pub user: u8,
    pub year: u16,
    /// The next round to play (ROUNDS = the season is over).
    pub round: u8,
    pub players: [PlayerRec; PLAYERS],
    /// Club of each player.
    pub club_of: [u8; PLAYERS],
    pub clubs: [ClubState; TEAMS],
    pub table: [Row; TEAMS],
    pub form: Form,
    /// The last round's income and wages for the user's club (the club
    /// screen's finance panel).
    pub last_income: i32,
    pub last_wages: i32,
    pub seed: u32,
    /// League titles won, per club (this career).
    pub titles: [u8; TEAMS],
}

impl Career {
    /// A fresh career managing club `user`.
    pub fn new(user: u8, seed: u32) -> Career {
        let mut players = [team::generate_player(0, 0); PLAYERS];
        let mut club_of = [0u8; PLAYERS];
        for c in 0..TEAMS {
            for s in 0..SQUAD {
                players[c * SQUAD + s] = team::generate_player(c, s);
                club_of[c * SQUAD + s] = c as u8;
            }
        }
        let clubs = core::array::from_fn(|c| ClubState {
            budget: START_BUDGET,
            morale: 55,
            formation: CLUBS[c].tactics.formation,
        });
        Career {
            user,
            year: FIRST_YEAR,
            round: 0,
            players,
            club_of,
            clubs,
            table: [Row::default(); TEAMS],
            form: [0; 5],
            last_income: 0,
            last_wages: 0,
            seed,
            titles: [0; TEAMS],
        }
    }

    /// Player indices of club `c`, in database order.
    pub fn squad(&self, c: u8) -> SquadIter<'_> {
        SquadIter {
            career: self,
            club: c,
            next: 0,
        }
    }

    pub fn squad_len(&self, c: u8) -> usize {
        self.club_of.iter().filter(|&&k| k == c).count()
    }

    /// The club's tactics with its chosen formation.
    pub fn tactics(&self, c: u8) -> Tactics {
        Tactics {
            formation: self.clubs[c as usize].formation,
            ..CLUBS[c as usize].tactics
        }
    }

    /// The best eleven for the club's formation: each slot (keeper first)
    /// takes the best remaining player for its role, tired players
    /// (energy < 45) only when nobody else is left.
    pub fn lineup(&self, c: u8) -> [u16; 11] {
        let f = FORMATIONS[self.clubs[c as usize].formation as usize % FORMATIONS.len()];
        let mut used = [false; PLAYERS];
        let mut out = [0u16; 11];
        for slot in 0..11 {
            let role = f.role(slot);
            let mut best: Option<(usize, i32)> = None;
            for i in self.squad(c) {
                if used[i] {
                    continue;
                }
                let p = &self.players[i];
                let keeper = p.pos == Pos::Gk;
                if keeper != (role == Role::Gk) {
                    continue;
                }
                let mut score = ovr_for(&p.attrs, role) as i32 * 4 + p.form as i32 * 2;
                if p.energy < 45 {
                    score -= 200;
                }
                if best.is_none_or(|(_, b)| score > b) {
                    best = Some((i, score));
                }
            }
            // A squad without a keeper (sold) still fields one: anyone.
            let pick = best
                .map(|(i, _)| i)
                .or_else(|| self.squad(c).find(|&i| !used[i]));
            if let Some(i) = pick {
                used[i] = true;
                out[slot] = i as u16;
            }
        }
        out
    }

    /// Match-ready attributes of player `i` (form applies; energy too).
    pub fn match_attrs(&self, i: usize) -> [u8; ATTRS] {
        let p = &self.players[i];
        let tired = (100 - p.energy.min(100) as i32) / 8;
        core::array::from_fn(|a| (p.attrs[a] as i32 + p.form as i32 - tired).clamp(1, 99) as u8)
    }

    /// The match setup for `home` v `away` with their lineups.
    pub fn setup(
        &self,
        home: u8,
        away: u8,
        half_ticks: u32,
        seed: u32,
        humans: [bool; 2],
        difficulty: u8,
        wet: bool,
    ) -> Setup {
        let side = |c: u8| {
            let xi = self.lineup(c);
            SideSetup {
                club: c,
                tactics: self.tactics(c),
                players: core::array::from_fn(|k| {
                    let i = xi[k] as usize;
                    (i as u16, self.players[i].number, self.match_attrs(i))
                }),
                morale: self.clubs[c as usize].morale,
                bench: self.bench(c, &xi),
                bench_n: self
                    .squad(c)
                    .filter(|i| !xi.contains(&(*i as u16)))
                    .count()
                    .min(BENCH) as u8,
            }
        };
        Setup {
            sides: [side(home), side(away)],
            half_ticks,
            seed,
            wet,
            humans,
            keeper_ctl: [false, false],
            difficulty,
        }
    }

    /// The best five not in the eleven (fresher players first).
    pub fn bench(&self, c: u8, xi: &[u16; 11]) -> [super::engine::BenchSlot; BENCH] {
        let mut out = [(0u16, 0u8, [0u8; ATTRS], false); BENCH];
        let mut rest = [0u16; MAX_SQUAD];
        let mut n = 0;
        for i in self.squad(c) {
            if !xi.contains(&(i as u16)) && n < rest.len() {
                rest[n] = i as u16;
                n += 1;
            }
        }
        rest[..n].sort_unstable_by_key(|&i| {
            let p = &self.players[i as usize];
            (core::cmp::Reverse(p.ovr() as u32 + p.energy as u32 / 4), i)
        });
        for (k, &i) in rest[..n.min(BENCH)].iter().enumerate() {
            let p = &self.players[i as usize];
            out[k] = (i, p.number, self.match_attrs(i as usize), p.pos == Pos::Gk);
        }
        out
    }

    /// A club's strength for the quick sim: the lineup's mean overall,
    /// nudged by morale.
    pub fn strength(&self, c: u8) -> i32 {
        let f = FORMATIONS[self.clubs[c as usize].formation as usize % FORMATIONS.len()];
        let xi = self.lineup(c);
        let mut s = 0i32;
        for (k, &i) in xi.iter().enumerate() {
            s += ovr_for(&self.match_attrs(i as usize), f.role(k)) as i32;
        }
        s / 11 + (self.clubs[c as usize].morale as i32 - 50) / 10
    }

    /// The round's fixtures (home, away): the circle method, so every pair
    /// meets once in [`ROUNDS`] rounds.
    pub fn fixtures(round: usize) -> [(u8, u8); PER_ROUND] {
        fixtures(round)
    }

    /// The user's fixture this round: (home, away).
    pub fn user_fixture(&self) -> Option<(u8, u8)> {
        if self.round as usize >= ROUNDS {
            return None;
        }
        fixtures(self.round as usize)
            .into_iter()
            .find(|&(h, a)| h == self.user || a == self.user)
    }

    /// Simulate a match between `home` and `away` without playing it.
    pub fn quick_score(&self, home: u8, away: u8, rng: &mut Rng) -> (u8, u8) {
        let sh = self.strength(home);
        let sa = self.strength(away);
        // Expected goals (×1000) from the strength gap; home edge.
        let lam = |s: i32, o: i32, home: bool| -> u32 {
            let base = 1_350 + (s - o) * 60 + if home { 150 } else { 0 };
            base.clamp(250, 3_600) as u32
        };
        let goals = |l: u32, rng: &mut Rng| -> u8 {
            // Nine chances, each scoring with l / 9: a binomial close to
            // Poisson for these rates.
            let mut g = 0;
            for _ in 0..9 {
                if rng.below(9_000) < l {
                    g += 1;
                }
            }
            g
        };
        (
            goals(lam(sh, sa, true), rng),
            goals(lam(sa, sh, false), rng),
        )
    }

    /// Finish the current round: `user_score` is the user's result (home,
    /// away goals) if played, else it is simulated too. Updates the table,
    /// money, morale, form and energy, and advances the round.
    pub fn finish_round(&mut self, user_score: Option<(u8, u8)>) -> [(u8, u8, u8, u8); PER_ROUND] {
        let round = self.round as usize;
        let mut rng =
            Rng::new(self.seed ^ (round as u32 + 1).wrapping_mul(0x9E37_79B9) ^ self.year as u32);
        let mut results = [(0, 0, 0, 0); PER_ROUND];
        for (k, (h, a)) in fixtures(round).into_iter().enumerate() {
            let (gh, ga) = match user_score {
                Some(s) if h == self.user || a == self.user => s,
                _ => self.quick_score(h, a, &mut rng),
            };
            results[k] = (h, a, gh, ga);
            self.table[h as usize].add(gh, ga);
            self.table[a as usize].add(ga, gh);
            for (c, gf, gag, home) in [(h, gh, ga, true), (a, ga, gh, false)] {
                self.after_match(c, gf, gag, home, &mut rng);
                if c == self.user {
                    self.form.rotate_left(1);
                    self.form[4] = if gf > gag {
                        1
                    } else if gf == gag {
                        2
                    } else {
                        3
                    };
                }
            }
        }
        // Everyone rests a little between rounds.
        for p in self.players.iter_mut() {
            p.energy = (p.energy as u32 + 18).min(100) as u8;
        }
        self.round += 1;
        if self.round as usize == ROUNDS {
            // The champion: a title and the prize money.
            let champ = self.standings()[0] as usize;
            self.titles[champ] = self.titles[champ].saturating_add(1);
            self.clubs[champ].budget += CHAMPION_PRIZE;
        }
        results
    }

    fn after_match(&mut self, c: u8, gf: u8, ga: u8, home: bool, rng: &mut Rng) {
        let won = gf > ga;
        let drawn = gf == ga;
        // Money: gate receipts at home, prize money, wages.
        let fame = CLUBS[c as usize].fame as i32;
        let income = if home { 1_500 + fame * 230 } else { 600 }
            + if won {
                900
            } else if drawn {
                350
            } else {
                0
            };
        let wages: i32 = self.squad(c).map(|i| self.players[i].wage() as i32).sum();
        let st = &mut self.clubs[c as usize];
        st.budget += income - wages;
        st.morale = (st.morale as i32
            + if won {
                8
            } else if drawn {
                1
            } else {
                -6
            })
        .clamp(10, 95) as u8;
        if c == self.user {
            self.last_income = income;
            self.last_wages = wages;
        }
        // The eleven who played: tired, and their form moves.
        let xi = self.lineup(c);
        for &i in &xi {
            let p = &mut self.players[i as usize];
            p.energy = p
                .energy
                .saturating_sub(30 - (p.attrs[team::STAMINA] / 10).min(15));
            let swing = if won {
                1
            } else if drawn {
                0
            } else {
                -1
            };
            let jitter = rng.below(3) as i32 - 1;
            p.form = (p.form as i32 + swing + jitter).clamp(-5, 5) as i8;
        }
    }

    /// Clubs by points, goal difference, goals for (then index).
    pub fn standings(&self) -> [u8; TEAMS] {
        let mut order: [u8; TEAMS] = core::array::from_fn(|i| i as u8);
        order.sort_unstable_by(|&a, &b| {
            let (ra, rb) = (&self.table[a as usize], &self.table[b as usize]);
            rb.points()
                .cmp(&ra.points())
                .then(rb.gd().cmp(&ra.gd()))
                .then(rb.gf.cmp(&ra.gf))
                .then(a.cmp(&b))
        });
        order
    }

    /// A generated player's age in `year` (everyone ages a year a season,
    /// up to 40).
    pub fn age_in(generated: u8, year: u16) -> u8 {
        (generated as u16 + year.saturating_sub(FIRST_YEAR)).min(40) as u8
    }

    /// Start the next season: the table resets, money and squads carry on,
    /// everyone ages a year.
    pub fn new_season(&mut self) {
        self.year += 1;
        self.round = 0;
        self.table = [Row::default(); TEAMS];
        // Morale settles over the summer (else last season's champion
        // starts the next one on a streak it never earned).
        for c in self.clubs.iter_mut() {
            c.morale = ((c.morale as u32 + 55) / 2) as u8;
        }
        for p in self.players.iter_mut() {
            p.age = p.age.saturating_add(1).min(40);
            p.energy = 100;
        }
    }

    /// What the user's club pays for player `i` (the seller's markup).
    pub fn buy_price(&self, i: usize) -> i32 {
        (self.players[i].value() * 11 / 10) as i32
    }

    /// What another club pays the user for player `i`.
    pub fn sell_price(&self, i: usize) -> i32 {
        (self.players[i].value() * 9 / 10) as i32
    }

    /// Buy player `i` for the user's club.
    pub fn buy(&mut self, i: usize) -> Result<i32, TransferError> {
        let seller = self.club_of[i];
        if seller == self.user {
            return Err(TransferError::OwnPlayer);
        }
        let price = self.buy_price(i);
        if self.clubs[self.user as usize].budget < price {
            return Err(TransferError::NotEnoughCoins);
        }
        if self.squad_len(self.user) >= MAX_SQUAD {
            return Err(TransferError::SquadFull);
        }
        if seller != NO_CLUB && self.squad_len(seller) <= MIN_SQUAD {
            return Err(TransferError::SquadTooSmall);
        }
        self.clubs[self.user as usize].budget -= price;
        if seller != NO_CLUB {
            self.clubs[seller as usize].budget += price;
        }
        self.club_of[i] = self.user;
        self.players[i].number = self.free_number(self.user);
        Ok(price)
    }

    /// Sell the user's player `i` to the club that can best afford him.
    pub fn sell(&mut self, i: usize) -> Result<(u8, i32), TransferError> {
        if self.club_of[i] != self.user {
            return Err(TransferError::OwnPlayer);
        }
        if self.squad_len(self.user) <= MIN_SQUAD {
            return Err(TransferError::SquadTooSmall);
        }
        let price = self.sell_price(i);
        let buyer = (0..TEAMS as u8)
            .filter(|&c| {
                c != self.user
                    && self.clubs[c as usize].budget >= price
                    && self.squad_len(c) < MAX_SQUAD
            })
            .max_by_key(|&c| (self.clubs[c as usize].budget, c))
            .ok_or(TransferError::NoBuyer)?;
        self.clubs[buyer as usize].budget -= price;
        self.clubs[self.user as usize].budget += price;
        self.club_of[i] = buyer;
        self.players[i].number = self.free_number(buyer);
        Ok((buyer, price))
    }

    /// The lowest shirt number club `c` doesn't use (from 2).
    fn free_number(&self, c: u8) -> u8 {
        for n in 2..99u8 {
            if !self.squad(c).any(|i| self.players[i].number == n) {
                return n;
            }
        }
        99
    }

    // ---------------------------------------------------------- save

    /// The save image (≤ [`SAVE_MAX`] bytes). Players are stored as their
    /// differences from the generated database, so the layout is fixed:
    /// club, attributes, energy, form per player.
    pub fn save(&self, settings: [u8; SETTINGS_LEN], out: &mut [u8; SAVE_MAX]) -> usize {
        let mut w = Writer { buf: out, at: 0 };
        w.bytes(b"SE11");
        w.u8(SAVE_VERSION);
        w.bytes(&settings);
        w.u8(self.user);
        w.u16(self.year);
        w.u8(self.round);
        w.u32(self.seed);
        for c in &self.clubs {
            w.u32(c.budget as u32);
            w.u8(c.morale);
            w.u8(c.formation);
        }
        for r in &self.table {
            w.bytes(&[r.played, r.won, r.drawn, r.lost, r.gf, r.ga]);
        }
        w.bytes(&self.form);
        w.u32(self.last_income as u32);
        w.u32(self.last_wages as u32);
        for i in 0..PLAYERS {
            let p = &self.players[i];
            w.u8(self.club_of[i]);
            w.bytes(&p.attrs);
            w.u8(p.energy);
            w.u8(p.form as u8);
            w.u8(p.number);
            // Version 3: the look in the byte that held the age (derived
            // from the year: everyone ages together).
            w.u8(p.appearance().byte());
        }
        w.bytes(&self.titles);
        let sum = checksum(&w.buf[..w.at]);
        w.u16(sum);
        w.at
    }

    /// Restore a save (versions 1 to 3); also returns the
    /// settings bytes (a version 1 save's four, then zeros: the defaults).
    pub fn load(data: &[u8]) -> Option<(Career, [u8; SETTINGS_LEN])> {
        if data.len() < 6 || &data[..4] != b"SE11" || !(1..=SAVE_VERSION).contains(&data[4]) {
            return None;
        }
        let v1 = data[4] == 1;
        let v3 = data[4] >= 3;
        let n = if v1 { SAVE_LEN_V1 } else { SAVE_LEN };
        if data.len() < n {
            return None;
        }
        let stored = u16::from_le_bytes([data[n - 2], data[n - 1]]);
        if checksum(&data[..n - 2]) != stored {
            return None;
        }
        let mut r = Reader { buf: data, at: 5 };
        let mut settings = [0u8; SETTINGS_LEN];
        for b in settings.iter_mut().take(if v1 { 4 } else { SETTINGS_LEN }) {
            *b = r.u8();
        }
        let user = r.u8();
        if user as usize >= TEAMS {
            return None;
        }
        let year = r.u16();
        let round = r.u8();
        let seed = r.u32();
        let mut c = Career::new(user, seed);
        c.year = year;
        c.round = round.min(ROUNDS as u8);
        for k in 0..TEAMS {
            c.clubs[k].budget = r.u32() as i32;
            c.clubs[k].morale = r.u8();
            c.clubs[k].formation = r.u8() % FORMATIONS.len() as u8;
        }
        for k in 0..TEAMS {
            let b = [r.u8(), r.u8(), r.u8(), r.u8(), r.u8(), r.u8()];
            c.table[k] = Row {
                played: b[0],
                won: b[1],
                drawn: b[2],
                lost: b[3],
                gf: b[4],
                ga: b[5],
            };
        }
        for k in 0..5 {
            c.form[k] = r.u8();
        }
        c.last_income = r.u32() as i32;
        c.last_wages = r.u32() as i32;
        for i in 0..PLAYERS {
            c.club_of[i] = r.u8();
            for a in 0..ATTRS {
                c.players[i].attrs[a] = r.u8().clamp(team::ATTR_MIN, team::ATTR_MAX);
            }
            c.players[i].energy = r.u8().min(100);
            c.players[i].form = (r.u8() as i8).clamp(-5, 5);
            c.players[i].number = r.u8();
            let b = r.u8();
            if v3 {
                c.players[i].set_appearance(team::Appearance::from_byte(b));
                c.players[i].age = Career::age_in(c.players[i].age, year);
            } else {
                c.players[i].age = b;
            }
        }
        if !v1 {
            for t in c.titles.iter_mut() {
                *t = r.u8();
            }
        }
        Some((c, settings))
    }
}

/// The first season's year.
pub const FIRST_YEAR: u16 = 2026;

/// The league winner's prize (coins).
pub const CHAMPION_PRIZE: i32 = 6_000;

/// Version 2 added the 16 settings bytes (controls, keeper, graphics) and
/// the titles; version 3 keeps each player's look (skin, hair) instead of
/// his age, which follows from the year. Versions 1 and 2 still load.
pub const SAVE_VERSION: u8 = 3;
/// Settings bytes in a save (the game's `Settings::bytes`).
pub const SETTINGS_LEN: usize = 16;
/// Bytes a save uses (fixed layout).
pub const SAVE_LEN: usize =
    4 + 1 + SETTINGS_LEN + 1 + 2 + 1 + 4 + TEAMS * 6 + TEAMS * 6 + 5 + 8 + PLAYERS * 11 + TEAMS + 2;
/// A version 1 save: 4 settings bytes, no titles.
const SAVE_LEN_V1: usize = SAVE_LEN - (SETTINGS_LEN - 4) - TEAMS;
/// The N64's 16 Kbit EEPROM, less the engine's framing.
pub const SAVE_MAX: usize = 2_000;

const _: () = assert!(SAVE_LEN <= SAVE_MAX);

fn checksum(b: &[u8]) -> u16 {
    let mut a: u32 = 1;
    let mut s: u32 = 0;
    for &x in b {
        a = (a + x as u32) % 65_521;
        s = (s + a) % 65_521;
    }
    (s ^ (a << 3)) as u16
}

struct Writer<'a> {
    buf: &'a mut [u8; SAVE_MAX],
    at: usize,
}

impl Writer<'_> {
    fn u8(&mut self, v: u8) {
        self.buf[self.at] = v;
        self.at += 1;
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.u8(x);
        }
    }
    fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> u8 {
        let v = self.buf[self.at];
        self.at += 1;
        v
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes([self.u8(), self.u8()])
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes([self.u8(), self.u8(), self.u8(), self.u8()])
    }
}

/// Player indices of one club.
pub struct SquadIter<'a> {
    career: &'a Career,
    club: u8,
    next: usize,
}

impl Iterator for SquadIter<'_> {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        while self.next < PLAYERS {
            let i = self.next;
            self.next += 1;
            if self.career.club_of[i] == self.club {
                return Some(i);
            }
        }
        None
    }
}

/// Round `r`'s pairs by the circle method: club 0 fixed, the others
/// rotating. Club 0 alternates home and away by round, the other pairs by
/// their position in the circle: every club gets four or five home games.
pub fn fixtures(r: usize) -> [(u8, u8); PER_ROUND] {
    let n = TEAMS;
    let mut ring = [0u8; TEAMS];
    ring[0] = 0;
    for k in 1..n {
        ring[k] = (1 + (k - 1 + r) % (n - 1)) as u8;
    }
    core::array::from_fn(|k| {
        let a = ring[k];
        let b = ring[n - 1 - k];
        let first_home = if k == 0 {
            r.is_multiple_of(2)
        } else {
            k % 2 == 0
        };
        if first_home { (a, b) } else { (b, a) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pair_meets_once_and_every_club_plays_each_round() {
        let mut met = [[0u8; TEAMS]; TEAMS];
        let mut homes = [0u8; TEAMS];
        for r in 0..ROUNDS {
            let mut seen = [false; TEAMS];
            for (h, a) in fixtures(r) {
                assert_ne!(h, a);
                assert!(!seen[h as usize] && !seen[a as usize], "round {r}");
                seen[h as usize] = true;
                seen[a as usize] = true;
                met[h as usize][a as usize] += 1;
                met[a as usize][h as usize] += 1;
                homes[h as usize] += 1;
            }
        }
        for a in 0..TEAMS {
            for b in 0..TEAMS {
                assert_eq!(met[a][b], if a == b { 0 } else { 1 }, "{a} v {b}");
            }
            assert!(
                (4..=5).contains(&homes[a]),
                "home games balanced: {homes:?}"
            );
        }
    }

    #[test]
    fn a_season_fills_the_table_and_balances_the_books() {
        let mut c = Career::new(0, 99);
        let budgets: [i32; TEAMS] = core::array::from_fn(|k| c.clubs[k].budget);
        let mut goals = 0u32;
        for _ in 0..ROUNDS {
            for (_, _, gh, ga) in c.finish_round(None) {
                goals += gh as u32 + ga as u32;
            }
        }
        assert_eq!(c.round as usize, ROUNDS);
        assert!(c.user_fixture().is_none());
        let played: u32 = c.table.iter().map(|r| r.played as u32).sum();
        assert_eq!(played, (TEAMS * ROUNDS) as u32);
        let gf: u32 = c.table.iter().map(|r| r.gf as u32).sum();
        let ga: u32 = c.table.iter().map(|r| r.ga as u32).sum();
        assert_eq!(gf, ga);
        assert_eq!(gf, goals);
        // A football-like scoring rate: 1.5–4.5 goals a match.
        let per = goals as f32 / (ROUNDS * PER_ROUND) as f32;
        assert!((1.5..4.5).contains(&per), "{per}");
        // Money moved (gate receipts, wages), nobody hoards forever.
        assert!((0..TEAMS).any(|k| c.clubs[k].budget != budgets[k]));
        let s = c.standings();
        for w in s.windows(2) {
            let (a, b) = (&c.table[w[0] as usize], &c.table[w[1] as usize]);
            assert!(a.points() >= b.points());
        }
        assert_ne!(c.form, [0; 5]);
    }

    #[test]
    fn seasons_are_deterministic() {
        let run = || {
            let mut c = Career::new(3, 7);
            for _ in 0..ROUNDS {
                c.finish_round(None);
            }
            c
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn the_users_result_is_used_when_played() {
        let mut c = Career::new(0, 1);
        let (h, a) = c.user_fixture().unwrap();
        let res = c.finish_round(Some((4, 0)));
        let mine = res.iter().find(|r| r.0 == h && r.1 == a).unwrap();
        assert_eq!((mine.2, mine.3), (4, 0));
        let (gf, ga) = if h == 0 { (4, 0) } else { (0, 4) };
        assert_eq!(c.table[0].gf, gf);
        assert_eq!(c.table[0].ga, ga);
        assert_eq!(c.form[4], if gf > ga { 1 } else { 3 });
    }

    #[test]
    fn transfers_respect_money_and_squad_limits() {
        let mut c = Career::new(0, 1);
        // The cheapest player at another club.
        let cheap = (0..PLAYERS)
            .filter(|&i| c.club_of[i] != 0)
            .min_by_key(|&i| c.buy_price(i))
            .unwrap();
        let seller = c.club_of[cheap];
        let before = (c.clubs[0].budget, c.clubs[seller as usize].budget);
        let price = c.buy(cheap).unwrap();
        assert_eq!(c.club_of[cheap], 0);
        assert_eq!(c.clubs[0].budget, before.0 - price);
        assert_eq!(c.clubs[seller as usize].budget, before.1 + price);
        assert_eq!(c.squad_len(0), SQUAD + 1);
        assert_eq!(c.buy(cheap), Err(TransferError::OwnPlayer));
        // Broke: the dearest player is out of reach.
        c.clubs[0].budget = 100;
        let dear = (0..PLAYERS)
            .filter(|&i| c.club_of[i] != 0)
            .max_by_key(|&i| c.buy_price(i))
            .unwrap();
        assert_eq!(c.buy(dear), Err(TransferError::NotEnoughCoins));
        // Full squad.
        c.clubs[0].budget = 10_000_000;
        while c.squad_len(0) < MAX_SQUAD {
            let i = (0..PLAYERS)
                .find(|&i| c.club_of[i] != 0 && c.squad_len(c.club_of[i]) > MIN_SQUAD)
                .unwrap();
            c.buy(i).unwrap();
        }
        let i = (0..PLAYERS).find(|&i| c.club_of[i] != 0).unwrap();
        assert_eq!(c.buy(i), Err(TransferError::SquadFull));
        // Selling goes to a club that can pay; never below the minimum.
        let mine = c.squad(0).next().unwrap();
        let (buyer, got) = c.sell(mine).unwrap();
        assert_ne!(buyer, 0);
        assert_eq!(c.club_of[mine], buyer);
        assert!(got > 0);
        while c.squad_len(0) > MIN_SQUAD {
            let k = c.squad(0).next().unwrap();
            c.sell(k).unwrap();
        }
        let k = c.squad(0).next().unwrap();
        assert_eq!(c.sell(k), Err(TransferError::SquadTooSmall));
    }

    #[test]
    fn lineups_field_a_keeper_and_eleven_different_players() {
        let c = Career::new(0, 1);
        for club in 0..TEAMS as u8 {
            let xi = c.lineup(club);
            assert_eq!(c.players[xi[0] as usize].pos, Pos::Gk);
            for a in 0..11 {
                assert_eq!(c.club_of[xi[a] as usize], club);
                for b in a + 1..11 {
                    assert_ne!(xi[a], xi[b]);
                }
            }
        }
    }

    #[test]
    fn tired_players_rest_on_the_bench() {
        let mut c = Career::new(0, 1);
        let xi = c.lineup(0);
        let star = xi[9] as usize;
        c.players[star].energy = 20;
        assert!(!c.lineup(0).contains(&(star as u16)));
    }

    #[test]
    fn the_save_round_trips_and_fits_the_eeprom() {
        let mut c = Career::new(4, 1234);
        c.finish_round(Some((2, 1)));
        c.finish_round(None);
        let i = (0..PLAYERS)
            .filter(|&i| c.club_of[i] != 4)
            .min_by_key(|&i| c.buy_price(i))
            .unwrap();
        c.buy(i).unwrap();
        c.players[3].attrs[0] = 77;
        c.players[7].set_appearance(team::Appearance {
            skin: 7,
            hair: 4,
            style: 5,
        });
        c.titles[4] = 2;
        // A later season: the ages follow the year.
        c.new_season();
        let mut buf = [0u8; SAVE_MAX];
        let st: [u8; SETTINGS_LEN] = core::array::from_fn(|k| k as u8 + 1);
        let n = c.save(st, &mut buf);
        assert_eq!(n, SAVE_LEN);
        assert!(
            n <= 2048 - 48,
            "{n} bytes leave room for the engine's framing"
        );
        let (back, settings) = Career::load(&buf[..n]).unwrap();
        assert_eq!(settings, st);
        assert_eq!(back, c);
        // Corruption is caught.
        buf[100] ^= 0x40;
        assert!(Career::load(&buf[..n]).is_none());
        assert!(Career::load(b"nope").is_none());
    }

    /// Rewrite a version 3 image as version 1 or 2 did it: each player's
    /// age where version 3 keeps his look.
    fn with_ages(c: &Career, buf: &mut [u8; SAVE_MAX], n: usize, version: u8) {
        let players = 4 + 1 + SETTINGS_LEN + 1 + 2 + 1 + 4 + TEAMS * 12 + 5 + 8;
        for i in 0..PLAYERS {
            buf[players + i * 11 + 10] = c.players[i].age;
        }
        buf[4] = version;
        let sum = checksum(&buf[..n - 2]);
        buf[n - 2..n].copy_from_slice(&sum.to_le_bytes());
    }

    /// A version 2 save (before the looks) loads with its ages and the
    /// generated looks.
    #[test]
    fn a_version_2_save_still_loads() {
        let mut c = Career::new(1, 7);
        c.finish_round(None);
        c.players[9].age = 33;
        let mut buf = [0u8; SAVE_MAX];
        let n = c.save([0; SETTINGS_LEN], &mut buf);
        with_ages(&c, &mut buf, n, 2);
        let (back, _) = Career::load(&buf[..n]).unwrap();
        assert_eq!(back, c);
    }

    /// A save from before the controls page (version 1: four settings
    /// bytes, no titles) loads, its settings padded with zeros (defaults).
    #[test]
    fn a_version_1_save_still_loads() {
        let mut c = Career::new(2, 99);
        c.finish_round(Some((1, 1)));
        let mut buf = [0u8; SAVE_MAX];
        let n = c.save(core::array::from_fn(|k| k as u8 + 1), &mut buf);
        with_ages(&c, &mut buf, n, 3);
        // Rewrite it as version 1.
        let mut v1 = Vec::new();
        v1.extend_from_slice(b"SE11");
        v1.push(1);
        v1.extend_from_slice(&buf[5..9]);
        v1.extend_from_slice(&buf[5 + SETTINGS_LEN..n - 2 - TEAMS]);
        let sum = checksum(&v1);
        v1.extend_from_slice(&sum.to_le_bytes());
        assert_eq!(v1.len(), SAVE_LEN_V1);
        let (back, settings) = Career::load(&v1).unwrap();
        assert_eq!(&settings[..4], &[1, 2, 3, 4]);
        assert!(settings[4..].iter().all(|&b| b == 0));
        assert_eq!(back.titles, [0; TEAMS]);
        assert_eq!(back.table, c.table);
        assert_eq!(back.players, c.players);
    }

    #[test]
    fn the_champion_gets_a_title_and_the_prize() {
        let mut c = Career::new(0, 5);
        for _ in 0..ROUNDS - 1 {
            c.finish_round(None);
        }
        assert_eq!(c.titles, [0; TEAMS]);
        let before = c.clubs.map(|k| k.budget);
        c.finish_round(None);
        let champ = c.standings()[0] as usize;
        assert_eq!(c.titles.iter().map(|&t| t as u32).sum::<u32>(), 1);
        assert_eq!(c.titles[champ], 1);
        // The prize on top of the last round's books.
        let others: i32 = (0..TEAMS)
            .filter(|&k| k != champ)
            .map(|k| c.clubs[k].budget - before[k])
            .max()
            .unwrap();
        assert!(c.clubs[champ].budget - before[champ] >= CHAMPION_PRIZE - 3_000 + others.min(0));
        c.new_season();
        assert_eq!(c.titles[champ], 1, "titles carry over seasons");
    }

    #[test]
    fn a_new_season_resets_the_table_and_keeps_the_squads() {
        let mut c = Career::new(0, 1);
        for _ in 0..ROUNDS {
            c.finish_round(None);
        }
        let squads = c.club_of;
        let age = c.players[0].age;
        c.new_season();
        assert_eq!(c.round, 0);
        assert_eq!(c.table, [Row::default(); TEAMS]);
        assert_eq!(c.club_of, squads);
        assert_eq!(c.players[0].age, age + 1);
        assert_eq!(c.year, 2027);
    }
}
