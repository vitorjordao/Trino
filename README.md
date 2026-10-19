# Evidence for PR #87 (issue #58: the UI system)

Screens are rendered by the engine's UI runtime and the real PC renderer
(`cargo xtask ui shot`, the editor's preview, the self-test probes).

| Folder | What |
|---|---|
| `pc/` | The new project's options and controls screens on the PC (1280x720); the title at 4K and 21:9; Solid Eleven's pause menu before the change |
| `n64/` | Options on the N64 (Graphics and Controls hidden; 16x12 safe area); Solid Eleven's pause before (2 rows) and after (3 rows); the `ui layout` self-test screen |
| `3ds/` | Options and pause on the 3DS (the menu on the touch screen); Solid Eleven before; the self-test screen with its touch button |
| `web/` | The title on a phone in portrait; Chromium WebGL2 and WebGPU self-test runs |
| `editor/` | The editor's UI tab: the focus order, both 3DS screens, the inspector, a pt-BR overflow, the 960x600 layout |
| `logs/` | `ui check` on a new project; ares (N64, with and without the Expansion Pak) and Azahar (New and old 3DS) self-test steps, including `ui layout` |
