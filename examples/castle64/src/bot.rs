//! Bot que JOGA o castle64 de ponta a ponta com inputs reais — o harness de
//! "playtest" das três plataformas (teste de PC roda o jogo inteiro; os
//! self-tests de N64/3DS rodam a primeira fase completa).
//!
//! Waypoints são posições de PÉS no chão. Regras de avanço do script:
//! - passo atual pertence a um nível != nível atual → pula para o próximo
//!   passo daquele nível (portas e retornos de estrela trocam de nível);
//! - `star_count` subiu → pula o passo atual (a estrela do hub não troca de
//!   nível, então a troca de nível sozinha não bastaria);
//! - durante o StarGet o bot fica parado (o jogo congela mesmo).
//!
//! `no_std`, determinístico: mesma seed de mundo → mesmo playthrough.

use crate::levels::LEVELS;
use crate::{Castle64Game, PLAYER_HALF, State, tri_wave};
use trino_core::math3d::{cos, sin, sqrt};
use trino_core::{Button, InputState, Vec3};

/// Um passo do roteiro, associado ao nível em que faz sentido.
#[derive(Clone, Copy, Debug)]
pub enum Step {
    /// Anda até o waypoint (pés); avança quando chega perto.
    Walk(Vec3),
    /// Anda pulando sempre que estiver no chão (atravessa gaps/sobe).
    Hop(Vec3),
    /// Para e espera a plataforma móvel `index` chegar ao extremo
    /// (`low` = ponto `a`, senão `b`).
    WaitMover { index: usize, low: bool },
    /// Para e espera o inimigo `index` ficar a mais de `dist` de `from`
    /// (no plano XZ) — janela para pegar estrelas guardadas.
    WaitEnemyFar { index: usize, from: Vec3, dist: f32 },
}

#[derive(Clone, Copy, Debug)]
pub struct ScriptStep {
    pub level: usize,
    pub step: Step,
}

const fn walk(level: usize, x: f32, y: f32, z: f32) -> ScriptStep {
    ScriptStep {
        level,
        step: Step::Walk(Vec3::new(x, y, z)),
    }
}

const fn hop(level: usize, x: f32, y: f32, z: f32) -> ScriptStep {
    ScriptStep {
        level,
        step: Step::Hop(Vec3::new(x, y, z)),
    }
}

const fn wait_mover(level: usize, index: usize, low: bool) -> ScriptStep {
    ScriptStep {
        level,
        step: Step::WaitMover { index, low },
    }
}

const fn wait_enemy(level: usize, index: usize, x: f32, z: f32, dist: f32) -> ScriptStep {
    ScriptStep {
        level,
        step: Step::WaitEnemyFar {
            index,
            from: Vec3::new(x, 0.0, z),
            dist,
        },
    }
}

/// Hub → porta verde → Green Hills inteira até a estrela → volta ao hub.
/// (Prefixo do FULL_RUN; usado sozinho nos self-tests de console.)
pub const GREEN_RUN: &[ScriptStep] = &[
    // hub: spawn (0,0,-10) → porta verde em (-12.5, 0, 0)
    walk(0, -6.0, 0.0, -6.0),
    walk(0, -12.4, 0.0, 0.0),
    // green hills: spawn (0,0,0), plataformas subindo até a estrela
    walk(1, 2.0, 0.0, 0.6),
    hop(1, 7.0, 0.0, 1.2), // ilha A, desviando do goomba (patrulha em z=0)
    hop(1, 12.5, 1.0, 1.5),
    hop(1, 16.5, 2.0, 4.5),
    hop(1, 13.5, 3.0, 8.5),
    hop(1, 8.5, 4.0, 10.5),
    hop(1, 3.0, 5.0, 11.8),
    wait_enemy(1, 1, 4.0, 13.0, 1.2),
    walk(1, 4.0, 5.0, 13.0), // estrela → StarGet → hub
];

/// O jogo inteiro: 4 estrelas + porta dourada + sala do trono.
pub const FULL_RUN: &[ScriptStep] = &[
    // ---- hub → green hills (estrela 1) ----
    walk(0, -6.0, 0.0, -6.0),
    walk(0, -12.4, 0.0, 0.0),
    walk(1, 2.0, 0.0, 0.6),
    hop(1, 7.0, 0.0, 1.2),
    hop(1, 12.5, 1.0, 1.5),
    hop(1, 16.5, 2.0, 4.5),
    hop(1, 13.5, 3.0, 8.5),
    hop(1, 8.5, 4.0, 10.5),
    hop(1, 3.0, 5.0, 11.8),
    wait_enemy(1, 1, 4.0, 13.0, 1.2),
    walk(1, 4.0, 5.0, 13.0),
    // ---- hub → lava land (estrela 2) ----
    walk(0, 8.0, 0.0, -2.0),
    walk(0, 12.4, 0.0, 0.0),
    hop(2, 0.0, 1.0, -4.5),
    hop(2, 3.5, 1.0, -0.5),
    wait_mover(2, 0, true),
    hop(2, 6.0, 1.0, 3.0),
    wait_mover(2, 0, false),
    hop(2, 7.0, 1.0, 9.5),
    wait_enemy(2, 0, 4.5, 11.0, 2.5),
    walk(2, 4.5, 1.0, 11.3),
    wait_mover(2, 1, true),
    hop(2, 0.5, 1.0, 11.5),
    wait_mover(2, 1, false),
    hop(2, -6.0, 5.0, 11.5), // estrela na beirada do ledge
    // ---- hub → sky tower (estrela 3) ----
    walk(0, -5.0, 0.0, 6.0),
    walk(0, -5.0, 0.0, 10.2),
    walk(3, 1.5, 0.0, -1.5), // sai da linha de patrulha do goomba da base
    hop(3, 4.0, 1.4, 1.0),
    hop(3, 6.0, 2.8, 4.0),
    hop(3, 4.0, 4.2, 7.0),
    hop(3, 1.0, 5.6, 8.0),
    hop(3, -2.0, 7.0, 7.0),
    hop(3, -4.0, 8.4, 4.0),
    hop(3, -4.0, 9.8, 0.0),
    wait_mover(3, 0, true),
    hop(3, -3.0, 10.3, -4.0),
    wait_mover(3, 0, false),
    hop(3, 2.6, 15.0, -2.6), // canto da plataforma, longe da patrulha
    wait_enemy(3, 1, 1.0, -4.0, 1.5),
    walk(3, 1.0, 15.0, -4.0),
    // ---- hub: estrela do telhado (estrela 4) ----
    walk(0, 10.0, 0.0, 3.0),
    hop(0, 13.0, 1.0, 5.0),
    hop(0, 15.0, 2.0, 7.5),
    hop(0, 13.0, 3.0, 9.5),
    hop(0, 10.5, 4.0, 12.0),
    hop(0, 8.0, 5.0, 14.0),
    hop(0, 5.0, 6.0, 13.0),
    walk(0, 0.0, 6.0, 12.5), // exatamente na estrela (raio de coleta 0.9)
    // ---- hub → porta dourada → trono ----
    walk(0, 0.0, 0.0, -4.0),
    walk(0, 0.0, 0.0, 10.2),
    walk(4, 0.0, 0.0, 2.0),
];

pub struct Bot {
    script: &'static [ScriptStep],
    i: usize,
    /// Primeiro passo do nível atual — ponto de replay após uma morte.
    level_start_i: usize,
    cur_level: usize,
    frames_in_step: u32,
    last_stars: u32,
    jump_held: bool,
}

impl Bot {
    pub fn new(script: &'static [ScriptStep]) -> Self {
        Bot {
            script,
            i: 0,
            level_start_i: 0,
            cur_level: 0,
            frames_in_step: 0,
            last_stars: 0,
            jump_held: false,
        }
    }

    pub fn done(&self) -> bool {
        self.i >= self.script.len()
    }

    pub fn step_index(&self) -> usize {
        self.i
    }

    pub fn frames_in_step(&self) -> u32 {
        self.frames_in_step
    }

    fn feet(g: &Castle64Game) -> Vec3 {
        g.pos - Vec3::new(0.0, PLAYER_HALF.y, 0.0)
    }

    /// Há chão (top próximo do nível dos pés) logo à frente na direção do
    /// movimento? Falso = beirada → hora de pular.
    fn ground_ahead(g: &Castle64Game, feet: Vec3, dir: Vec3) -> bool {
        let probe = feet + dir * 0.55;
        let mut solids = [crate::EMPTY_SOLID; crate::MAX_SOLIDS];
        let n = g.build_solids(&mut solids);
        solids[..n].iter().any(|s| {
            let a = &s.aabb;
            // Lava não é chão: contar com ela fazia o bot entrar andando.
            !s.lava
                && probe.x > a.min.x - 0.05
                && probe.x < a.max.x + 0.05
                && probe.z > a.min.z - 0.05
                && probe.z < a.max.z + 0.05
                && a.max.y <= feet.y + 0.15
                && a.max.y >= feet.y - 1.2
        })
    }

    /// Parede baixa (topo alcançável com um pulo) bloqueando logo à frente?
    fn wall_ahead(g: &Castle64Game, feet: Vec3, dir: Vec3) -> bool {
        let probe = feet + dir * 0.6 + Vec3::new(0.0, 0.5, 0.0);
        let mut solids = [crate::EMPTY_SOLID; crate::MAX_SOLIDS];
        let n = g.build_solids(&mut solids);
        solids[..n].iter().any(|s| {
            let a = &s.aabb;
            !s.lava
                && probe.x > a.min.x
                && probe.x < a.max.x
                && probe.y > a.min.y
                && probe.y < a.max.y
                && probe.z > a.min.z
                && probe.z < a.max.z
                && a.max.y - feet.y <= 1.6
        })
    }

    /// Stick que move o player na direção `dir` (mundo), dado o yaw da
    /// câmera do jogo (inverso do mapeamento câmera-relativa do update).
    fn stick_toward(g: &Castle64Game, dir: Vec3) -> trino_core::Vec2 {
        let fwd = Vec3::new(-sin(g.cam_yaw), 0.0, -cos(g.cam_yaw));
        let right = Vec3::new(0.0, 1.0, 0.0).cross(fwd);
        trino_core::Vec2::new(dir.dot(right), dir.dot(fwd))
    }

    /// Próximo input. Avança o script internamente; `done()` sinaliza fim.
    pub fn drive(&mut self, g: &Castle64Game) -> InputState {
        let mut input = InputState::default();

        // Estrela em celebração: tudo parado (o update congela mesmo).
        if matches!(g.state, State::StarGet(_)) {
            self.frames_in_step = 0;
            return input;
        }

        // Estrela nova coletada: o passo atual cumpriu seu papel.
        let stars = g.star_count();
        if stars > self.last_stars {
            self.last_stars = stars;
            self.i += 1;
            self.frames_in_step = 0;
        }

        // Mudou de nível (porta ou retorno de estrela): pula para o próximo
        // passo do nível atual.
        if g.level != self.cur_level {
            while self.i < self.script.len() && self.script[self.i].level != g.level {
                self.i += 1;
            }
            self.cur_level = g.level;
            self.level_start_i = self.i;
            self.frames_in_step = 0;
        } else if g.level_time < 1.5 / 60.0 && self.i > self.level_start_i {
            // level_time zerado sem troca de nível = respawn (lava, queda,
            // inimigo): joga o trecho do nível de novo desde o começo.
            self.i = self.level_start_i;
            self.frames_in_step = 0;
        }
        if self.done() {
            return input;
        }

        self.frames_in_step += 1;
        let feet = Self::feet(g);
        match self.script[self.i].step {
            Step::Walk(wp) | Step::Hop(wp) => {
                let can_jump = matches!(self.script[self.i].step, Step::Hop(_));
                let flat = Vec3::new(wp.x - feet.x, 0.0, wp.z - feet.z);
                let dist = sqrt(flat.dot(flat));
                if dist < 0.5 && (feet.y - wp.y).abs() < 0.7 {
                    self.i += 1;
                    self.frames_in_step = 0;
                } else if g.on_ground && wp.y - feet.y > 1.7 {
                    // Caiu da rota num lugar que não mata (chão do hub, base
                    // da torre): waypoint inalcançável — rejoga o trecho.
                    self.i = self.level_start_i;
                    self.frames_in_step = 0;
                } else {
                    let dir = if dist > 1e-4 {
                        flat * (1.0 / dist)
                    } else {
                        Vec3::ZERO
                    };
                    // Com inércia, chegar a toda velocidade derrapa para
                    // fora dos pads pequenos: modula o stick pela distância
                    // (aterrissagens e pulos continuam a 100%).
                    let mag = if g.on_ground {
                        (dist / 1.8).clamp(0.45, 1.0)
                    } else {
                        1.0
                    };
                    input.stick = Self::stick_toward(g, dir * mag);
                    // Pula só quando precisa: beirada à frente (salto de
                    // alcance máximo) ou degrau mais alto ali perto —
                    // bunny-hop cego aterrissava no meio dos gaps.
                    // Já no ar: SEGURA o A (pulo variável — soltar corta).
                    if !g.on_ground && self.jump_held && g.vel.y > 0.0 {
                        input.set(Button::A, true);
                    }
                    let step_up = wp.y > feet.y + 0.2 && dist < 2.0;
                    if can_jump
                        && g.on_ground
                        && !self.jump_held
                        && (step_up
                            || Self::wall_ahead(g, feet, dir)
                            || !Self::ground_ahead(g, feet, dir))
                    {
                        input.set(Button::A, true);
                    }
                }
            }
            Step::WaitMover { index, low } => {
                let m = &LEVELS[g.level].movers[index];
                let phase = tri_wave(g.level_time / m.period);
                let ready = if low { phase < 0.06 } else { phase > 0.94 };
                if ready {
                    self.i += 1;
                    self.frames_in_step = 0;
                }
            }
            Step::WaitEnemyFar { index, from, dist } => {
                let e = g.enemies[index];
                let d = Vec3::new(e.pos.x - from.x, 0.0, e.pos.z - from.z);
                if !e.alive || d.dot(d) > dist * dist {
                    self.i += 1;
                    self.frames_in_step = 0;
                }
            }
        }
        // Alterna o A para gerar bordas de "pressionado" a cada pulo.
        self.jump_held = input.is_down(Button::A);
        input
    }
}
