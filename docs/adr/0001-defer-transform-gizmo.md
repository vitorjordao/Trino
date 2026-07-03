# ADR 0001 — Defer the viewport transform gizmo

**Status:** accepted (2026-07-03)

## Context

The obvious crate, `transform-gizmo-egui` (0.9.0), supports only egui 0.34.
Trino's editor is on egui 0.35 because egui-wgpu 0.35 is what pins our wgpu
version (29) for the whole engine. Adopting the gizmo would either hold the
entire editor on egui 0.34 (and a different wgpu) or force us to vendor and
port the gizmo now.

## Decision

Ship editor v1 **without** an in-viewport gizmo:

- transforms are edited in the Inspector (drag values);
- clicking a sprite in the viewport selects it (cheap tint highlight).

Revisit when either (a) `transform-gizmo-egui` catches up to our egui
version, or (b) Fase 7 (3D) makes a gizmo non-negotiable — then vendor it.

## Consequences

- No dependency holds our egui/wgpu upgrades hostage (the exact failure mode
  Fyrox/Bevy postmortems warn about).
- Positioning by drag-in-viewport is deferred; keyboard/inspector editing is
  the v1 workflow.
