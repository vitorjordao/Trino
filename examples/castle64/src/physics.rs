//! Física 3D do castle64: AABB contra AABB, resolvida eixo a eixo.
//!
//! Só aritmética `f32` pura — determinística em PC/N64/3DS, igual ao
//! `trino_core::collide` 2D. O chamador faz o substep (dt fixo pequeno).

use trino_core::Vec3;

/// Caixa alinhada aos eixos, em coordenadas de mundo (Y para cima).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Aabb { min, max }
    }

    pub fn from_center_half(center: Vec3, half: Vec3) -> Self {
        Aabb {
            min: center - half,
            max: center + half,
        }
    }

    #[inline]
    pub fn overlaps(&self, o: &Aabb) -> bool {
        self.min.x < o.max.x
            && o.min.x < self.max.x
            && self.min.y < o.max.y
            && o.min.y < self.max.y
            && self.min.z < o.max.z
            && o.min.z < self.max.z
    }
}

/// Resultado de um passo de movimento.
pub struct MoveOut {
    pub pos: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    pub hit_ceiling: bool,
    /// Índice (em `solids`) do sólido em que o player está apoiado.
    pub standing_on: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
    Z,
}

#[inline]
fn axis_get(v: Vec3, a: Axis) -> f32 {
    match a {
        Axis::X => v.x,
        Axis::Y => v.y,
        Axis::Z => v.z,
    }
}

#[inline]
fn axis_set(v: &mut Vec3, a: Axis, val: f32) {
    match a {
        Axis::X => v.x = val,
        Axis::Y => v.y = val,
        Axis::Z => v.z = val,
    }
}

/// Pele de contato: o teste de overlap de cada eixo encolhe os OUTROS
/// eixos por esta margem. Sem ela, um player pousado com os pés a 1e-7
/// dentro do chão (imprecisão de f32 em `top + half - half`) era tratado
/// como colisão HORIZONTAL e teleportado para a face lateral do bloco —
/// achado pelo bot de playtest ao aterrissar correndo numa ilha.
const SKIN: f32 = 0.02;

/// Move `pos` (centro) por `vel * dt` contra `solids`, um eixo por vez
/// (X, Z e por último Y, para detectar apoio no chão de forma estável).
pub fn move_aabb(pos: Vec3, half: Vec3, vel: Vec3, dt: f32, solids: &[Aabb]) -> MoveOut {
    let mut out = MoveOut {
        pos,
        vel,
        on_ground: false,
        hit_ceiling: false,
        standing_on: None,
    };

    for axis in [Axis::X, Axis::Z, Axis::Y] {
        let dv = axis_get(vel, axis) * dt;
        if dv == 0.0 && axis != Axis::Y {
            continue;
        }
        let new = axis_get(out.pos, axis) + dv;
        axis_set(&mut out.pos, axis, new);
        let skin = match axis {
            Axis::Y => Vec3::new(SKIN, 0.0, SKIN),
            _ => Vec3::new(0.0, SKIN, 0.0),
        };
        let me = Aabb::new(out.pos - half + skin, out.pos + half - skin);
        for (i, s) in solids.iter().enumerate() {
            if !me.overlaps(s) {
                continue;
            }
            match axis {
                Axis::Y => {
                    if dv <= 0.0 {
                        // Caindo/apoiado: sobe até o topo do sólido.
                        axis_set(&mut out.pos, Axis::Y, s.max.y + half.y);
                        out.vel.y = 0.0;
                        out.on_ground = true;
                        out.standing_on = Some(i);
                    } else {
                        axis_set(&mut out.pos, Axis::Y, s.min.y - half.y);
                        out.vel.y = 0.0;
                        out.hit_ceiling = true;
                    }
                }
                _ => {
                    if dv > 0.0 {
                        axis_set(
                            &mut out.pos,
                            axis,
                            axis_get(s.min, axis) - axis_get(half, axis),
                        );
                    } else if dv < 0.0 {
                        axis_set(
                            &mut out.pos,
                            axis,
                            axis_get(s.max, axis) + axis_get(half, axis),
                        );
                    }
                    match axis {
                        Axis::X => out.vel.x = 0.0,
                        Axis::Z => out.vel.z = 0.0,
                        Axis::Y => {}
                    }
                }
            }
        }
    }
    out
}

/// Distância do primeiro hit do raio `origin + dir*t` (dir normalizado)
/// contra as caixas, limitada a `max_t` (retorna `max_t` sem hit).
/// Slab method — usado pela câmera para não atravessar geometria.
pub fn raycast_aabbs(origin: Vec3, dir: Vec3, max_t: f32, boxes: &[Aabb]) -> f32 {
    let mut nearest = max_t;
    for b in boxes {
        let mut t_enter = 0.0f32;
        let mut t_exit = nearest;
        let mut hit = true;
        for axis in 0..3 {
            let (o, d, min, max) = match axis {
                0 => (origin.x, dir.x, b.min.x, b.max.x),
                1 => (origin.y, dir.y, b.min.y, b.max.y),
                _ => (origin.z, dir.z, b.min.z, b.max.z),
            };
            if d.abs() < 1e-6 {
                if o < min || o > max {
                    hit = false;
                    break;
                }
                continue;
            }
            let (mut t0, mut t1) = ((min - o) / d, (max - o) / d);
            if t0 > t1 {
                core::mem::swap(&mut t0, &mut t1);
            }
            t_enter = t_enter.max(t0);
            t_exit = t_exit.min(t1);
            if t_enter > t_exit {
                hit = false;
                break;
            }
        }
        if hit && t_enter < nearest {
            nearest = t_enter.max(0.0);
        }
    }
    nearest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor() -> [Aabb; 1] {
        [Aabb::new(
            Vec3::new(-10.0, -1.0, -10.0),
            Vec3::new(10.0, 0.0, 10.0),
        )]
    }

    #[test]
    fn falls_and_lands_on_floor() {
        let solids = floor();
        let mut pos = Vec3::new(0.0, 3.0, 0.0);
        let mut vel = Vec3::ZERO;
        let half = Vec3::new(0.3, 0.5, 0.3);
        let mut grounded = false;
        for _ in 0..300 {
            vel.y -= 25.0 * (1.0 / 120.0);
            let out = move_aabb(pos, half, vel, 1.0 / 120.0, &solids);
            pos = out.pos;
            vel = out.vel;
            if out.on_ground {
                grounded = true;
                break;
            }
        }
        assert!(grounded);
        assert!((pos.y - 0.5).abs() < 1e-4, "pos.y = {}", pos.y);
    }

    #[test]
    fn wall_blocks_horizontal_movement() {
        let solids = [
            floor()[0],
            Aabb::new(Vec3::new(2.0, 0.0, -1.0), Vec3::new(3.0, 2.0, 1.0)),
        ];
        let mut pos = Vec3::new(0.0, 0.5, 0.0);
        for _ in 0..240 {
            let out = move_aabb(
                pos,
                Vec3::new(0.3, 0.5, 0.3),
                Vec3::new(5.0, -1.0, 0.0),
                1.0 / 120.0,
                &solids,
            );
            pos = out.pos;
        }
        assert!((pos.x - 1.7).abs() < 1e-3, "pos.x = {}", pos.x);
    }

    #[test]
    fn landing_on_top_never_snags_on_the_side() {
        // Pouso com pés "1e-7 dentro" do topo (imprecisão de f32): andar
        // sobre o bloco não pode teleportar para a face lateral.
        let solids = [Aabb::new(
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(10.0, 1.0, 3.0),
        )];
        let half = Vec3::new(0.3, 0.55, 0.3);
        // 1.55 - 0.55 = 0.99999994f32 < 1.0 — o caso real do playtest.
        let mut pos = Vec3::new(2.0, 1.55, 1.5);
        let mut min_x = pos.x;
        for _ in 0..240 {
            let out = move_aabb(pos, half, Vec3::new(5.0, -2.0, 0.0), 1.0 / 120.0, &solids);
            assert!(
                out.pos.x >= min_x - 1e-4,
                "snag lateral: x {} -> {}",
                min_x,
                out.pos.x
            );
            min_x = out.pos.x;
            pos = out.pos;
            if pos.x > 8.0 {
                break;
            }
        }
        assert!(pos.x > 8.0, "não avançou sobre o bloco: {}", pos.x);
    }

    #[test]
    fn raycast_hits_the_nearest_box() {
        let boxes = [
            Aabb::new(Vec3::new(4.0, -1.0, -1.0), Vec3::new(6.0, 1.0, 1.0)),
            Aabb::new(Vec3::new(2.0, -1.0, -1.0), Vec3::new(3.0, 1.0, 1.0)),
        ];
        let t = raycast_aabbs(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 10.0, &boxes);
        assert!((t - 2.0).abs() < 1e-4, "t = {t}");
        // Sem hit: devolve max_t.
        let t = raycast_aabbs(Vec3::ZERO, Vec3::new(-1.0, 0.0, 0.0), 10.0, &boxes);
        assert_eq!(t, 10.0);
        // Eixo com direção zero fora do slab: sem hit.
        let t = raycast_aabbs(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            10.0,
            &boxes,
        );
        assert_eq!(t, 10.0);
    }

    #[test]
    fn ceiling_stops_upward_motion() {
        let solids = [Aabb::new(
            Vec3::new(-1.0, 2.0, -1.0),
            Vec3::new(1.0, 3.0, 1.0),
        )];
        let out = move_aabb(
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.3, 0.5, 0.3),
            Vec3::new(0.0, 10.0, 0.0),
            0.1,
            &solids,
        );
        assert!(out.hit_ceiling);
        assert_eq!(out.vel.y, 0.0);
        assert!((out.pos.y - 1.5).abs() < 1e-4);
    }
}
