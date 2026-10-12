//! Scene-arranging operations behind the editor's *Arrange* menu and the
//! command palette: align, distribute, array, drop to surface, group moves.
//! Pure functions over a [`Scene`] (no UI), so every one is unit-tested and
//! an AI agent gets exactly what a click does — positions in the RON file.
//!
//! Positions are handled in **world space** and written back as local
//! transforms through the parent's inverse, so entities under rotated or
//! scaled parents land where the operation says.

use trino_core::{Aabb, Mat34, Vec3};
use trino_scene::{Entity, Scene};

use crate::gizmo::inverse;

/// World matrix of the entity at `path` (parents' transforms applied).
pub fn world_matrix(scene: &Scene, path: &[usize]) -> Option<Mat34> {
    let mut m = Mat34::IDENTITY;
    for i in 1..=path.len() {
        m = m.mul(&scene.get(&path[..i])?.transform.matrix());
    }
    Some(m)
}

pub fn world_position(scene: &Scene, path: &[usize]) -> Option<Vec3> {
    world_matrix(scene, path).map(|m| m.transform_point(Vec3::ZERO))
}

/// Move the entity at `path` so its origin lands on world point `p`.
pub fn set_world_position(scene: &mut Scene, path: &[usize], p: Vec3) -> bool {
    let parent = match path.split_last() {
        Some((_, [])) => Mat34::IDENTITY,
        Some((_, parent)) => match world_matrix(scene, parent) {
            Some(m) => m,
            None => return false,
        },
        None => return false,
    };
    let local = inverse(&parent).transform_point(p);
    match scene.get_mut(path) {
        Some(e) => {
            e.transform.pos = [local.x, local.y, local.z];
            true
        }
        None => false,
    }
}

fn axis(v: Vec3, a: usize) -> f32 {
    match a {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

fn with_axis(mut v: Vec3, a: usize, value: f32) -> Vec3 {
    match a {
        0 => v.x = value,
        1 => v.y = value,
        _ => v.z = value,
    }
    v
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignTo {
    Min,
    Center,
    Max,
}

/// Align the origins of `paths` on world `axis` (0 = X, 1 = Y, 2 = Z) to
/// their minimum, center or maximum. Returns how many moved.
pub fn align(scene: &mut Scene, paths: &[Vec<usize>], ax: usize, to: AlignTo) -> usize {
    let pts: Vec<(Vec<usize>, Vec3)> = paths
        .iter()
        .filter_map(|p| world_position(scene, p).map(|w| (p.clone(), w)))
        .collect();
    if pts.len() < 2 {
        return 0;
    }
    let values = pts.iter().map(|(_, w)| axis(*w, ax));
    let (lo, hi) = values.fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(v), h.max(v)));
    let target = match to {
        AlignTo::Min => lo,
        AlignTo::Center => (lo + hi) * 0.5,
        AlignTo::Max => hi,
    };
    pts.into_iter()
        .filter(|(p, w)| set_world_position(scene, p, with_axis(*w, ax, target)))
        .count()
}

/// Space the origins of `paths` evenly on world `axis` between the two
/// outermost (which stay put). Order along the axis is preserved.
pub fn distribute(scene: &mut Scene, paths: &[Vec<usize>], ax: usize) -> usize {
    let mut pts: Vec<(Vec<usize>, Vec3)> = paths
        .iter()
        .filter_map(|p| world_position(scene, p).map(|w| (p.clone(), w)))
        .collect();
    if pts.len() < 3 {
        return 0;
    }
    pts.sort_by(|a, b| axis(a.1, ax).total_cmp(&axis(b.1, ax)));
    let (lo, hi) = (axis(pts[0].1, ax), axis(pts[pts.len() - 1].1, ax));
    let step = (hi - lo) / (pts.len() - 1) as f32;
    let n = pts.len();
    pts.into_iter()
        .enumerate()
        .filter(|(i, (p, w))| {
            let v = if *i == n - 1 {
                hi
            } else {
                lo + step * *i as f32
            };
            set_world_position(scene, p, with_axis(*w, ax, v))
        })
        .count()
}

/// Duplicate the entity at `path` `count` times as its next siblings, each
/// copy `step` further (local units of the parent) than the previous one;
/// names are made unique. Returns the new paths.
pub fn array(scene: &mut Scene, path: &[usize], count: usize, step: [f32; 3]) -> Vec<Vec<usize>> {
    let Some(src) = scene.get(path).cloned() else {
        return Vec::new();
    };
    let Some(&at) = path.last() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 1..=count {
        let mut e: Entity = src.clone();
        e.name = scene.unique_name(&src.name);
        for (k, s) in step.iter().enumerate() {
            e.transform.pos[k] += s * i as f32;
        }
        let Some(list) = scene.siblings_mut(path) else {
            break;
        };
        list.insert(at + i, e);
        let mut p = path.to_vec();
        *p.last_mut().unwrap() = at + i;
        out.push(p);
    }
    out
}

/// Drop the entity at `path` straight down onto the highest surface below
/// its lowest point: `own` is the entity's world box (its drawn parts) and
/// `others` the world boxes of everything else. With nothing below, it
/// lands on the ground plane (y = 0). Returns the new world Y of the origin.
pub fn drop_to_surface(
    scene: &mut Scene,
    path: &[usize],
    own: Option<Aabb>,
    others: &[Aabb],
) -> Option<f32> {
    let origin = world_position(scene, path)?;
    // Height of the origin above the entity's own bottom.
    let (bottom, foot) = match own {
        Some(b) => (b.min.y, origin.y - b.min.y),
        None => (origin.y, 0.0),
    };
    let (x0, x1, z0, z1) = match own {
        Some(b) => (b.min.x, b.max.x, b.min.z, b.max.z),
        None => (origin.x, origin.x, origin.z, origin.z),
    };
    // Highest top among boxes under the footprint, not above the bottom.
    let surface = others
        .iter()
        .filter(|o| o.min.x <= x1 && o.max.x >= x0 && o.min.z <= z1 && o.max.z >= z0)
        .map(|o| o.max.y)
        .filter(|&top| top <= bottom + 1e-3)
        .fold(0.0f32, f32::max);
    let y = surface + foot;
    set_world_position(scene, path, Vec3::new(origin.x, y, origin.z)).then_some(y)
}

/// Remove several entities (paths may be nested or siblings): deepest and
/// last first so earlier paths stay valid. Returns the removed names.
pub fn remove_many(scene: &mut Scene, paths: &[Vec<usize>]) -> Vec<String> {
    let mut sorted: Vec<&Vec<usize>> = paths.iter().collect();
    sorted.sort();
    sorted.dedup();
    // Skip descendants of an entity that is itself being removed.
    let roots: Vec<&Vec<usize>> = sorted
        .iter()
        .filter(|p| !sorted.iter().any(|q| q.len() < p.len() && p.starts_with(q)))
        .copied()
        .collect();
    let mut out = Vec::new();
    for p in roots.into_iter().rev() {
        if let Some(e) = scene.remove(p) {
            out.push(e.name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Scene {
        let mut s = Scene::new("t");
        s.entities.push(Entity::new("a").at([0.0, 0.0, 0.0]));
        s.entities.push(Entity::new("b").at([4.0, 1.0, 2.0]));
        s.entities.push(Entity::new("c").at([10.0, 3.0, -1.0]));
        let mut parent = Entity::new("p").at([100.0, 0.0, 0.0]);
        parent.transform.scale = [2.0, 2.0, 2.0];
        parent.children.push(Entity::new("kid").at([1.0, 0.0, 0.0]));
        s.entities.push(parent);
        s
    }

    fn all() -> Vec<Vec<usize>> {
        vec![vec![0], vec![1], vec![2]]
    }

    #[test]
    fn world_positions_compose_parents() {
        let s = scene();
        assert_eq!(
            world_position(&s, &[3, 0]),
            Some(Vec3::new(102.0, 0.0, 0.0))
        );
    }

    #[test]
    fn set_world_position_goes_through_the_parent() {
        let mut s = scene();
        assert!(set_world_position(
            &mut s,
            &[3, 0],
            Vec3::new(110.0, 4.0, 0.0)
        ));
        assert_eq!(s.get(&[3, 0]).unwrap().transform.pos, [5.0, 2.0, 0.0]);
    }

    #[test]
    fn align_and_distribute() {
        let mut s = scene();
        assert_eq!(align(&mut s, &all(), 1, AlignTo::Max), 3);
        assert!(
            all()
                .iter()
                .all(|p| s.get(p).unwrap().transform.pos[1] == 3.0)
        );
        let mut s = scene();
        align(&mut s, &all(), 0, AlignTo::Center);
        assert!(
            all()
                .iter()
                .all(|p| s.get(p).unwrap().transform.pos[0] == 5.0)
        );
        let mut s = scene();
        assert_eq!(distribute(&mut s, &all(), 0), 3);
        assert_eq!(
            s.get(&[1]).unwrap().transform.pos[0],
            5.0,
            "middle centered"
        );
        assert_eq!(s.get(&[2]).unwrap().transform.pos[0], 10.0, "ends stay");
        assert_eq!(distribute(&mut s, &all()[..2], 0), 0, "needs three");
    }

    #[test]
    fn array_makes_offset_named_copies() {
        let mut s = scene();
        let new = array(&mut s, &[1], 3, [0.0, 0.0, 2.0]);
        assert_eq!(new, vec![vec![2], vec![3], vec![4]]);
        assert_eq!(s.get(&[4]).unwrap().transform.pos, [4.0, 1.0, 8.0]);
        let names: Vec<&str> = s.entities.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names.len(), 7);
        assert_eq!(
            names.iter().filter(|n| n.starts_with('b')).count(),
            4,
            "{names:?}"
        );
        assert_eq!(
            names.iter().collect::<std::collections::HashSet<_>>().len(),
            7,
            "unique names: {names:?}"
        );
    }

    #[test]
    fn drop_lands_on_the_highest_surface_below() {
        let mut s = scene();
        // `c` floats at y = 3 with a 1 m box whose bottom is its origin.
        let own = Aabb::new(Vec3::new(9.5, 3.0, -1.5), Vec3::new(10.5, 4.0, -0.5));
        let table = Aabb::new(Vec3::new(9.0, 0.0, -2.0), Vec3::new(11.0, 1.0, 0.0));
        let roof = Aabb::new(Vec3::new(9.0, 5.0, -2.0), Vec3::new(11.0, 6.0, 0.0));
        let far = Aabb::new(Vec3::new(50.0, 0.0, 50.0), Vec3::new(51.0, 2.5, 51.0));
        assert_eq!(
            drop_to_surface(&mut s, &[2], Some(own), &[table, roof, far]),
            Some(1.0)
        );
        assert_eq!(s.get(&[2]).unwrap().transform.pos[1], 1.0);
        // Nothing under it: the ground plane.
        assert_eq!(drop_to_surface(&mut s, &[1], None, &[far]), Some(0.0));
    }

    #[test]
    fn remove_many_keeps_paths_valid() {
        let mut s = scene();
        let gone = remove_many(&mut s, &[vec![0], vec![2], vec![3], vec![3, 0]]);
        assert_eq!(gone.len(), 3, "the kid goes with its parent: {gone:?}");
        assert_eq!(s.entities.len(), 1);
        assert_eq!(s.entities[0].name, "b");
    }
}
