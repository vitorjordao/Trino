//! Layout audit for the UI tests: no widget may run off the window
//! sideways, and no two widgets may sit on top of each other. Runs over the
//! accessibility tree egui_kittest exposes, so it sees every label, button,
//! check box, combo and field a user would — in any panel, at any window
//! size the test picks.
//!
//! What it catches: text that does not fit a narrow inspector and pushes a
//! button out of view, a row whose widgets overlap after a label grew, a
//! menu that opens past the window's edge. What it cannot see: text egui
//! truncates on purpose (with "…"), and painter-only overlays (no node).

use eframe::egui::{self, accesskit::Role};
use egui_kittest::kittest::{NodeT, Queryable};

/// Roles of the widgets the audit checks (leaves a user reads or clicks).
fn audited(role: Role) -> bool {
    matches!(
        role,
        Role::Label
            | Role::Button
            | Role::CheckBox
            | Role::RadioButton
            | Role::ComboBox
            | Role::TextInput
            | Role::MultilineTextInput
            | Role::SpinButton
            | Role::Slider
            | Role::Link
            | Role::Tab
            | Role::MenuItem
    )
}

#[derive(Clone, Debug)]
pub struct Widget {
    pub role: Role,
    pub label: String,
    /// Where it is laid out (may run past its panel: that is a bug).
    pub rect: egui::Rect,
    /// The part its clip region shows (overlaps are judged on this).
    pub visible: egui::Rect,
}

/// Every audited widget on screen (with a non-empty rect). With `ctx`,
/// widgets a scroll area or a panel clips away entirely are left out, and
/// `visible` is the part that shows (accessibility rects are unclipped).
pub fn widgets<'t, Q, N>(q: &'t Q, ctx: Option<&egui::Context>) -> Vec<Widget>
where
    Q: Queryable<'t, 't, N>,
    N: NodeT<'t> + 't,
{
    // egui's own widget rects carry the clipped `interact_rect`; match them
    // to accessibility nodes by their (unclipped) rect.
    let mut clipped: Vec<(egui::Rect, egui::Rect)> = Vec::new();
    if let Some(ctx) = ctx {
        ctx.viewport(|v| {
            for pass in [&v.prev_pass, &v.this_pass] {
                for (_, rects) in pass.widgets.layers() {
                    for w in rects {
                        clipped.push((w.rect, w.interact_rect));
                    }
                }
            }
        });
    }
    q.query_all_by(|n| audited(n.role()) && n.children().next().is_none())
        .filter_map(|n| {
            let ak = n.accesskit_node();
            let b = ak.bounding_box()?;
            let rect = egui::Rect::from_min_max(
                egui::pos2(b.x0 as f32, b.y0 as f32),
                egui::pos2(b.x1 as f32, b.y1 as f32),
            );
            let same = |a: egui::Rect| {
                (a.min - rect.min).length() < 0.5 && (a.max - rect.max).length() < 0.5
            };
            let visible = clipped
                .iter()
                .find(|(r, _)| same(*r))
                .map_or(rect, |(_, c)| rect.intersect(*c));
            (rect.width() > 0.5
                && rect.height() > 0.5
                && visible.width() > 0.5
                && visible.height() > 0.5)
                .then(|| Widget {
                    role: ak.role(),
                    label: ak.label().unwrap_or_default(),
                    rect,
                    visible,
                })
        })
        .collect()
}

/// Problems with `widgets` inside a window of `size` (logical points):
/// widgets past the left/right edge (or the bottom, unless `scrolls`), and
/// pairs overlapping by more than a sliver. Empty = clean.
pub fn problems(widgets: &[Widget], size: egui::Vec2, scrolls: bool) -> Vec<String> {
    const SLACK: f32 = 1.0;
    let mut out = Vec::new();
    for w in widgets {
        let r = w.rect;
        if r.min.x < -SLACK || r.max.x > size.x + SLACK {
            out.push(format!(
                "{:?} `{}` runs off the window sideways: x {:.0}..{:.0} of {:.0} (y {:.0})",
                w.role, w.label, r.min.x, r.max.x, size.x, r.min.y
            ));
        }
        if !scrolls && (r.min.y < -SLACK || r.max.y > size.y + SLACK) {
            out.push(format!(
                "{:?} `{}` runs off the window vertically: y {:.0}..{:.0} of {:.0}",
                w.role, w.label, r.min.y, r.max.y, size.y
            ));
        }
    }
    for (i, a) in widgets.iter().enumerate() {
        for b in &widgets[i + 1..] {
            let o = a.visible.intersect(b.visible);
            // Touching edges and 1-2 px antialias overlaps are fine.
            if o.width() > 2.0 && o.height() > 2.0 {
                out.push(format!(
                    "{:?} `{}` {:?} overlaps {:?} `{}` {:?}",
                    a.role, a.label, a.visible, b.role, b.label, b.visible
                ));
            }
        }
    }
    out
}

/// Asserts a clean layout of a harness, listing every problem.
pub fn assert_clean<S>(
    h: &egui_kittest::Harness<'_, S>,
    size: egui::Vec2,
    scrolls: bool,
    what: &str,
) {
    let w = widgets(h, Some(&h.ctx));
    assert!(!w.is_empty(), "{what}: no widgets found");
    let p = problems(&w, size, scrolls);
    if !p.is_empty() && std::env::var("TRINO_LAYOUT_DUMP").is_ok() {
        for x in &w {
            eprintln!("{:?} `{}` {:?}", x.role, x.label, x.rect);
        }
    }
    assert!(p.is_empty(), "{what}: layout problems:\n{}", p.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(label: &str, x0: f32, x1: f32) -> Widget {
        let rect = egui::Rect::from_min_max(egui::pos2(x0, 0.0), egui::pos2(x1, 20.0));
        Widget {
            role: Role::Button,
            label: label.into(),
            rect,
            visible: rect,
        }
    }

    #[test]
    fn flags_overflow_and_overlap_only() {
        let size = egui::vec2(100.0, 50.0);
        assert!(problems(&[w("a", 0.0, 40.0), w("b", 41.0, 99.0)], size, false).is_empty());
        let p = problems(&[w("a", 0.0, 60.0), w("b", 50.0, 120.0)], size, false);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p[0].contains("`b` runs off") && p[1].contains("overlaps"));
    }
}
