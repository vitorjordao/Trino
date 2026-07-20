//! Dados dos níveis do castle64 — tudo `const`, zero alocação.
//!
//! Convenções: Y para cima; posições de spawn/portas/inimigos são "pés no
//! chão" (o jogo converte para centro do AABB). Blocos são `min + size`.

use core::f32::consts::PI;
use trino_core::{Color, Vec3};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockKind {
    Grass,
    Stone,
    Brick,
    Castle,
    Roof,
    Lava,
}

#[derive(Clone, Copy, Debug)]
pub struct Block {
    pub min: Vec3,
    pub size: Vec3,
    pub kind: BlockKind,
}

/// Plataforma móvel: `min` oscila entre `a` e `b` (onda triangular).
#[derive(Clone, Copy, Debug)]
pub struct Mover {
    pub size: Vec3,
    pub a: Vec3,
    pub b: Vec3,
    pub period: f32,
    pub kind: BlockKind,
}

/// Inimigo patrulhando de `a` até `b` (pés no chão), ida e volta.
#[derive(Clone, Copy, Debug)]
pub struct EnemyDef {
    pub a: Vec3,
    pub b: Vec3,
    pub speed: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DoorColor {
    Green,
    Red,
    Blue,
    Gold,
}

#[derive(Clone, Copy, Debug)]
pub struct Portal {
    /// Pés da porta (base, centro).
    pub pos: Vec3,
    /// Rotação Y em radianos (a face da porta aponta para +Z rotacionado).
    pub yaw: f32,
    /// Índice do nível de destino em `LEVELS`.
    pub dest: usize,
    /// Estrelas necessárias para abrir.
    pub need: u8,
    pub color: DoorColor,
}

pub struct Level {
    /// Nome para diagnósticos (usado pelos testes).
    #[allow(dead_code)]
    pub name: &'static str,
    pub blocks: &'static [Block],
    pub movers: &'static [Mover],
    pub enemies: &'static [EnemyDef],
    pub coins: &'static [Vec3],
    /// Posição da estrela do nível (centro), se houver.
    pub star: Option<Vec3>,
    /// Bit em `stars` que a estrela deste nível marca.
    pub star_bit: u8,
    pub portals: &'static [Portal],
    /// Pés do spawn.
    pub spawn: Vec3,
    /// Yaw inicial da câmera (PI = câmera ao sul olhando para +Z).
    pub spawn_yaw: f32,
    pub kill_y: f32,
    pub sky: Color,
}

const fn blk(x: f32, y: f32, z: f32, w: f32, h: f32, d: f32, kind: BlockKind) -> Block {
    Block {
        min: Vec3::new(x, y, z),
        size: Vec3::new(w, h, d),
        kind,
    }
}

const fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

// ---------------------------------------------------------------- hub ----

const HUB_BLOCKS: &[Block] = &[
    // Chão gramado do pátio.
    blk(-16.0, -2.0, -16.0, 32.0, 2.0, 32.0, BlockKind::Grass),
    // Castelo: muralha principal + telhado + torres.
    blk(-7.0, 0.0, 11.0, 14.0, 5.0, 4.0, BlockKind::Castle),
    blk(-7.5, 5.0, 10.5, 15.0, 1.0, 5.0, BlockKind::Roof),
    blk(-9.0, 0.0, 10.0, 2.0, 7.0, 2.0, BlockKind::Castle),
    blk(7.0, 0.0, 10.0, 2.0, 7.0, 2.0, BlockKind::Castle),
    blk(-9.2, 7.0, 9.8, 2.4, 1.0, 2.4, BlockKind::Roof),
    blk(6.8, 7.0, 9.8, 2.4, 1.0, 2.4, BlockKind::Roof),
    // Escada de tijolos até o telhado (estrela secreta do hub). Degraus
    // quase adjacentes: os vãos diagonais de 3.0 originais eram injogáveis
    // com subida de +1 (playtest do bot).
    blk(12.0, 0.0, 4.0, 2.0, 1.0, 2.0, BlockKind::Brick),
    blk(14.0, 0.0, 6.5, 2.0, 2.0, 2.0, BlockKind::Brick),
    blk(12.0, 0.0, 8.5, 2.0, 3.0, 2.0, BlockKind::Brick),
    blk(9.5, 0.0, 11.0, 2.0, 4.0, 2.0, BlockKind::Brick),
    blk(7.0, 0.0, 13.0, 2.0, 5.0, 2.0, BlockKind::Brick),
    // Ilhas decorativas com moedas.
    blk(-13.0, 0.0, -8.0, 2.0, 1.0, 2.0, BlockKind::Brick),
    blk(-9.0, 0.0, -12.0, 2.0, 2.0, 2.0, BlockKind::Brick),
];

const HUB_COINS: &[Vec3] = &[
    v(0.0, 0.6, -4.0),
    v(-2.0, 0.6, -2.0),
    v(2.0, 0.6, -2.0),
    v(-12.0, 1.6, -7.0),
    v(-8.0, 2.6, -11.0),
    v(13.0, 1.6, 5.0),
    v(12.9, 3.6, 9.5),
    v(8.0, 5.6, 14.0),
];

const HUB_PORTALS: &[Portal] = &[
    Portal {
        pos: v(-12.5, 0.0, 0.0),
        yaw: PI * 0.5,
        dest: 1,
        need: 0,
        color: DoorColor::Green,
    },
    Portal {
        pos: v(12.5, 0.0, 0.0),
        yaw: -PI * 0.5,
        dest: 2,
        need: 1,
        color: DoorColor::Red,
    },
    // Na fachada oeste do castelo (a porta atrás do spawn tampava a câmera).
    Portal {
        pos: v(-5.0, 0.0, 10.6),
        yaw: PI,
        dest: 3,
        need: 2,
        color: DoorColor::Blue,
    },
    Portal {
        pos: v(0.0, 0.0, 10.6),
        yaw: PI,
        dest: 4,
        need: 4,
        color: DoorColor::Gold,
    },
];

const HUB_ENEMIES: &[EnemyDef] = &[EnemyDef {
    a: v(-6.0, 0.0, -4.0),
    b: v(-2.0, 0.0, -4.0),
    speed: 1.5,
}];

pub const HUB: Level = Level {
    name: "hub",
    blocks: HUB_BLOCKS,
    movers: &[],
    enemies: HUB_ENEMIES,
    coins: HUB_COINS,
    star: Some(v(0.0, 6.8, 12.5)),
    star_bit: 1 << 0,
    portals: HUB_PORTALS,
    spawn: v(0.0, 0.0, -10.0),
    spawn_yaw: PI,
    kill_y: -10.0,
    sky: Color::rgb(100, 160, 255),
};

// ------------------------------------------------------- green hills ----

const GREEN_BLOCKS: &[Block] = &[
    blk(-4.0, -2.0, -4.0, 8.0, 2.0, 8.0, BlockKind::Grass),
    blk(5.0, -2.0, -2.0, 4.0, 2.0, 4.0, BlockKind::Grass),
    // Gap 1.5 (era 2.0): com o movimento por inércia, gap 2.0 + subida 1.0
    // ficava no limite exato do alcance do pulo.
    blk(10.5, -1.0, 0.0, 3.5, 2.0, 3.0, BlockKind::Grass),
    blk(15.0, 0.0, 3.0, 3.0, 2.0, 3.0, BlockKind::Grass),
    blk(12.0, 1.0, 7.0, 3.0, 2.0, 3.0, BlockKind::Grass),
    blk(7.0, 2.0, 9.0, 3.0, 2.0, 3.0, BlockKind::Grass),
    blk(2.0, 3.0, 11.0, 4.0, 2.0, 4.0, BlockKind::Grass),
];

const GREEN_COINS: &[Vec3] = &[
    v(2.0, 0.6, 0.0),
    v(6.5, 0.6, 0.0),
    v(12.5, 1.6, 1.5),
    v(16.5, 2.6, 4.5),
    v(13.5, 3.6, 8.5),
    v(8.5, 4.6, 10.5),
    v(4.0, 5.6, 13.0),
];

const GREEN_ENEMIES: &[EnemyDef] = &[
    EnemyDef {
        a: v(5.5, 0.0, 0.0),
        b: v(8.5, 0.0, 0.0),
        speed: 1.6,
    },
    EnemyDef {
        a: v(3.0, 5.0, 12.0),
        b: v(5.0, 5.0, 14.0),
        speed: 1.2,
    },
];

pub const GREEN: Level = Level {
    name: "green-hills",
    blocks: GREEN_BLOCKS,
    movers: &[],
    enemies: GREEN_ENEMIES,
    coins: GREEN_COINS,
    star: Some(v(4.0, 5.9, 13.0)),
    star_bit: 1 << 1,
    portals: &[],
    spawn: v(0.0, 0.0, 0.0),
    spawn_yaw: -PI * 0.5,
    kill_y: -8.0,
    sky: Color::rgb(120, 200, 255),
};

// --------------------------------------------------------- lava land ----

const LAVA_BLOCKS: &[Block] = &[
    // Piscina de lava (letal ao toque).
    blk(-14.0, -2.0, -14.0, 28.0, 2.0, 28.0, BlockKind::Lava),
    blk(-2.0, 0.0, -12.0, 4.0, 1.0, 4.0, BlockKind::Stone),
    blk(-1.5, 0.0, -6.0, 3.0, 1.0, 3.0, BlockKind::Stone),
    blk(2.0, 0.0, -2.0, 3.0, 1.0, 3.0, BlockKind::Stone),
    blk(4.0, 0.0, 8.0, 6.0, 1.0, 6.0, BlockKind::Stone),
    // Ledge da estrela: 1.0 mais largo na direção do elevador — o gap de
    // 3.0 original era injogável (playtest do bot: o elevador sobe ~1.0
    // enquanto o pulo cruza o vão).
    blk(-8.0, 3.0, 9.0, 5.0, 2.0, 5.0, BlockKind::Stone),
];

const LAVA_MOVERS: &[Mover] = &[
    Mover {
        size: v(3.0, 0.5, 3.0),
        a: v(4.5, 0.5, 1.5),
        b: v(4.5, 0.5, 4.5),
        period: 4.0,
        kind: BlockKind::Stone,
    },
    // Elevador 1.0 mais perto da ilha (gap 2.0) — idem playtest.
    Mover {
        size: v(3.0, 0.5, 3.0),
        a: v(-1.0, 0.5, 10.0),
        b: v(-1.0, 4.5, 10.0),
        period: 5.0,
        kind: BlockKind::Stone,
    },
];

const LAVA_COINS: &[Vec3] = &[
    v(0.0, 1.6, -6.5),
    v(3.5, 1.6, -0.5),
    v(6.0, 1.8, 3.0),
    v(7.0, 1.6, 11.0),
    v(-0.5, 3.0, 11.5),
    v(-6.0, 5.6, 10.0),
];

const LAVA_ENEMIES: &[EnemyDef] = &[EnemyDef {
    a: v(5.0, 1.0, 9.5),
    b: v(9.0, 1.0, 12.5),
    speed: 1.8,
}];

pub const LAVA: Level = Level {
    name: "lava-land",
    blocks: LAVA_BLOCKS,
    movers: LAVA_MOVERS,
    enemies: LAVA_ENEMIES,
    coins: LAVA_COINS,
    star: Some(v(-6.0, 5.9, 11.5)),
    star_bit: 1 << 2,
    portals: &[],
    spawn: v(0.0, 1.0, -10.0),
    spawn_yaw: PI,
    kill_y: -6.0,
    sky: Color::rgb(255, 120, 70),
};

// --------------------------------------------------------- sky tower ----

const SKY_BLOCKS: &[Block] = &[
    blk(-3.0, -2.0, -3.0, 6.0, 2.0, 6.0, BlockKind::Stone),
    blk(3.0, 0.4, 0.0, 2.0, 1.0, 2.0, BlockKind::Brick),
    blk(5.0, 1.8, 3.0, 2.0, 1.0, 2.0, BlockKind::Brick),
    blk(3.0, 3.2, 6.0, 2.0, 1.0, 2.0, BlockKind::Brick),
    blk(0.0, 4.6, 7.0, 2.0, 1.0, 2.0, BlockKind::Brick),
    blk(-3.0, 6.0, 6.0, 2.0, 1.0, 2.0, BlockKind::Brick),
    blk(-5.0, 7.4, 3.0, 2.0, 1.0, 2.0, BlockKind::Brick),
    blk(-5.0, 8.8, -1.0, 2.0, 1.0, 2.0, BlockKind::Brick),
    blk(-1.0, 14.0, -6.0, 4.0, 1.0, 4.0, BlockKind::Stone),
];

const SKY_MOVERS: &[Mover] = &[Mover {
    size: v(2.0, 0.5, 2.0),
    a: v(-4.0, 9.8, -5.0),
    b: v(-4.0, 14.3, -5.0),
    period: 5.0,
    kind: BlockKind::Brick,
}];

const SKY_COINS: &[Vec3] = &[
    v(4.0, 2.0, 1.0),
    v(6.0, 3.4, 4.0),
    v(4.0, 4.8, 7.0),
    v(1.0, 6.2, 8.0),
    v(-2.0, 7.6, 7.0),
    v(-4.0, 9.0, 4.0),
    v(-4.0, 10.4, 0.0),
];

const SKY_ENEMIES: &[EnemyDef] = &[
    EnemyDef {
        a: v(-1.5, 0.0, -1.5),
        b: v(1.5, 0.0, 1.5),
        speed: 1.4,
    },
    EnemyDef {
        a: v(0.0, 15.0, -5.5),
        b: v(2.0, 15.0, -3.5),
        speed: 1.2,
    },
];

pub const SKY: Level = Level {
    name: "sky-tower",
    blocks: SKY_BLOCKS,
    movers: SKY_MOVERS,
    enemies: SKY_ENEMIES,
    coins: SKY_COINS,
    star: Some(v(1.0, 15.9, -4.0)),
    star_bit: 1 << 3,
    portals: &[],
    spawn: v(0.0, 0.0, 0.0),
    spawn_yaw: -PI * 0.5,
    kill_y: -6.0,
    sky: Color::rgb(150, 170, 230),
};

// ------------------------------------------------------- throne room ----

// Sala grande o bastante para a câmera orbital (dist 7) ficar DENTRO dela.
const THRONE_BLOCKS: &[Block] = &[
    blk(-12.0, -1.0, -12.0, 24.0, 1.0, 24.0, BlockKind::Castle),
    blk(-12.0, 0.0, 11.0, 24.0, 4.0, 1.0, BlockKind::Castle),
    blk(-12.0, 0.0, -12.0, 24.0, 4.0, 1.0, BlockKind::Castle),
    blk(11.0, 0.0, -12.0, 1.0, 4.0, 24.0, BlockKind::Castle),
    blk(-12.0, 0.0, -12.0, 1.0, 4.0, 24.0, BlockKind::Castle),
];

const THRONE_COINS: &[Vec3] = &[
    v(-3.0, 0.6, 0.0),
    v(3.0, 0.6, 0.0),
    v(0.0, 0.6, 3.0),
    v(-2.1, 0.6, -2.1),
    v(2.1, 0.6, -2.1),
    v(-2.1, 0.6, 2.1),
    v(2.1, 0.6, 2.1),
];

pub const THRONE: Level = Level {
    name: "throne",
    blocks: THRONE_BLOCKS,
    movers: &[],
    enemies: &[],
    coins: THRONE_COINS,
    star: None,
    star_bit: 0,
    portals: &[],
    spawn: v(0.0, 0.0, -4.0),
    spawn_yaw: PI,
    kill_y: -5.0,
    sky: Color::rgb(255, 215, 120),
};

pub const LEVELS: [&Level; 5] = [&HUB, &GREEN, &LAVA, &SKY, &THRONE];

/// Total de estrelas do jogo (para a porta dourada).
pub const TOTAL_STARS: u8 = 4;
