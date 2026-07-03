# Trino — Plano de Execução Passo a Passo

> Engine Rust multiplataforma: **Nintendo 64 / Nintendo 3DS / PC**, com editor visual,
> simulação de consoles, live reload, testes em emuladores, CI/CD e repositório AI-friendly.
>
> Este documento transforma a arquitetura de `PLANO_ENGINE_TRINO.md` em um roadmap
> executável. Toda versão de ferramenta citada foi verificada em **julho de 2026**.

---

## 1. Decisões de projeto

| Decisão | Escolha | Racional |
|---|---|---|
| Editor | **egui in-process** (modelo Fyrox) | Viewport wgpu render-to-texture; sem IPC; hot reload direto |
| Sequência | **2D primeiro nos 3 alvos, 3D como fase obrigatória depois** | Valida a abstração inteira antes da parte mais difícil (RSP/citro3d) |
| Nome | **Trino** | Placeholder barato de renomear |
| Build N64 no Windows | **Docker** (container oficial libdragon) | Único caminho suportado sem WSL manual; transparente via `xtask` |
| Licença | **MIT OR Apache-2.0** | Convenção Rust/Bevy |
| Direção de dependências | `core` não conhece nada; `game` → só `core`; `platform-*` → só `core`; `apps/*` colam | Regra inviolável — CI valida com `cargo deny` / teste de grafo |

**Teto de design = N64.** Materiais são presets (enum), não shaders livres. O PC pode
mais, mas a engine só expõe o que os três alvos conseguem — o modo estrito valida isso.

---

## 2. Stack verificada (julho/2026)

### N64
| Item | Ferramenta / versão | Notas |
|---|---|---|
| Runner cargo | **nust64 0.4.1** | Empacota IPL3 open-source do libdragon; flags `--libdragon debug\|release`, `--post-exec` |
| Target | `mips-nintendo64-none.json` custom + **nightly-2025-01-12** + `-Zbuild-std=core,alloc` | Modelo: `rust-n64/n64-project-template`; rustflags `-Clto -Cembed-bitcode` |
| SDK C | **libdragon** via Docker `ghcr.io/dragonminded/libdragon` | `:trunk` estável (pinar digest); `:preview` necessário p/ tiny3d/mkmodel (3D) |
| Bindings | **Camada bindgen própria** contra libdragon pinado | `sarchar/libdragon-rs` está 18 meses parado — não usar |
| Assets | `mksprite`→`.sprite`, `mkfont`→`.font64`, `mkmodel` (glTF)→`.model64`, `audioconv64`→`.wav64/.xm64`, `mkdfs`→`.dfs` anexado à ROM | Todos dentro do container |
| 3D | **tiny3d** (branch preview) | Lib 3D de facto sobre rdpq |
| Windows | Docker Desktop + npm CLI `libdragon` v12.2.1 | `xtask` chama por baixo |

### Emuladores N64 — nenhum faz tudo; estratégia de 3 emuladores
| Emulador | Uso | Como |
|---|---|---|
| **ares v148** | Verdade de acurácia + ISViewer | Homebrew Mode imprime `debugf`; GUI-only → `xvfb` no CI; CLI `ares --system "Nintendo 64" rom.z64 --no-file-prompt`; sem IPC de reload → kill+relaunch |
| **mupen64plus-ui-console** | Golden screenshots | `--testshots N,M --sshotdir DIR --nospeedlimit` (tira screenshots nos frames e sai); atenção: RSP HLE pode renderizar errado microcode rdpq |
| **cen64** | Testes de texto headless | `-headless -is-viewer`; lento, projeto semi-dormente — só fallback |

**Protocolo de teste**: magic strings via ISViewer (buffer `0xB3FF0020`, padrão do
n64-systemtest) + timeout externo + kill. `TEST_PASS`/`TEST_FAIL:<detalhe>` → exit code.

**Hardware real**: UNFLoader v2.2 (`-r rom.z64 -l` re-upload automático ao mudar, `-d`
debug, `-g` GDB) e SummerCart64 `sc64deployer`.

### 3DS
| Item | Ferramenta / versão | Notas |
|---|---|---|
| Bindings | **ctru-rs** (ativo, git-dep apenas) | gfx/ndsp/romfs/hid/fs/soc |
| 3D | **citro3d-rs** (WIP ativo) | `include_shader!` roda picasso em compile time |
| 2D | **FFI próprio para citro2d** | Não existem bindings oficiais |
| Cargo | **cargo-3ds 0.1.5** | `new/build/run/test`; saída `.3dsx`; `--address` usa 3dslink; `--server` streama stdout; runner custom OK; **não gera CIA** (makerom depois) |
| Target | `armv6k-nintendo-3ds` (Tier 3) nightly + build-std + devkitARM | devkitPro grupo pacman `3ds-dev`; Windows tem instalador gráfico oficial |
| CI | imagem `devkitpro/devkitarm:20260610` | libctru/citro3d/citro2d **pré-instalados** em `/opt/devkitpro` |
| CI runner | **rust3ds/actions** `setup@v1` + `run-tests@v1` | xvfb + Mesa + fork do Citra (r0c2f076) com GDB-stub→exit codes + `--dump-video` webm; testes com romfs não suportados |
| Emulador local | **Azahar 2125.1.3** | Qt-only, sem headless; launch por CLI historicamente bugado (issue #2066); ship de core libretro → RetroArch UDP porta 55355 aceita RESET/QUIT |

### PC / Editor
| Item | Ferramenta / versão | Notas |
|---|---|---|
| Render | **wgpu** + WGSL | |
| UI editor | **egui 0.35.0 + eframe** | Viewport: `egui_wgpu::Renderer::register_native_texture(device, view, FilterMode::Nearest)` → `TextureId` → `ui.image`; **recriar+re-registrar no resize** senão vaza GPU |
| Docking | **egui_dock 0.20.1** (tabs estilo Unity) | Alternativa: egui_tiles 0.16 |
| Gizmo | **transform-gizmo-egui 0.9.0** | ⚠️ só suporta egui **0.34** — gizmo trava upgrades do egui; pinar ou vendorar |
| Asset browser | **Fazer o nosso** (não existe crate) | Listas virtualizadas + egui_extras |
| Perf | mimalloc, puffin_egui, repaint-on-demand | |

### Live reload
| Item | Ferramenta / versão | Notas |
|---|---|---|
| Código | **hot-lib-reloader 0.8.2** | crate game com `crate-type=["rlib","dylib"]`; funções `#[unsafe(no_mangle)]` recebendo `&mut State` (estado é do host); `LibReloadObserver` (`wait_for_about_to_reload`/`wait_for_reload`) p/ migração serde do estado |
| Assets | **notify 8.x + notify-debouncer-full** | debounce 50–200 ms (cobre write+rename do Windows); watch masters → rebake → swap in-place por handle IDs estáveis (design do FileWatcher do Bevy) |
| Watch | **`cargo xtask watch <plataforma>`** próprio | cargo-watch foi arquivado (jan/2025) |
| Flags | `RUSTFLAGS="-C prefer-dynamic"` + profile `dev-hot-reload`; tudo atrás da feature `reload` | Consoles/release linkam estático |

Armadilhas conhecidas do hot reload (documentar no AGENTS.md): drift de assinatura ou
layout = UB; sem generics na fronteira; statics e `TypeId` resetam (registry de tipos
chaveado por nome/hash); `tracing` incompatível.

### Simulação de console no PC (WGSL)
| Console | Técnica |
|---|---|
| **N64** | Filtro 3-point bilinear **no material** (3 fetches de texel — não é pós-processo; portar Shadertoy `Ws2fWV`); quantização RGBA5551 + dither ordenado magic-square/Bayer (ref. GLideN64 PR #2183); estágio VI opcional (dedither/divot/edge-AA, ref. parallel-rdp); framebuffer offscreen 320×240 + upscale nearest inteiro (sharp-bilinear p/ frações) |
| **3DS** | 400×240 (tela de cima), restrições de material fixed-function, bilinear comum, cor 24-bit |
| **PC-estrito** | Valida `Caps` do N64 em debug: excedeu TMEM/limites → erro imediato |

Editor mostra "look mode" (aproximação); screenshots de emulador no CI são a verdade.

### CI/CD
| Item | Ferramenta / versão |
|---|---|
| Toolchain action | `dtolnay/rust-toolchain` |
| Cache | `Swatinem/rust-cache@v2` (v2.9.1, chaves por target) |
| Runners | `ubuntu-24.04`, `windows-2025`, `macos-latest` (**pinar labels** — imagens migrando em 2026) |
| Emulador em CI | `apt-get install xvfb` + `xvfb-run -a` + Mesa software GL |
| Release | auto-tag da versão do workspace + `softprops/action-gh-release@v3` com `generate_release_notes` |
| Depois | `release-plz` quando publicar em crates.io; `cargo-dist` 0.32 p/ instaladores |

---

## 3. Estrutura do repositório

```
trino/
├── crates/
│   ├── core/            # traits (Renderer/Audio/Input/Platform) + math; no_std; zero deps
│   ├── game-api/        # ABI estável p/ hot reload (tipos repr(C), fronteira dylib)
│   ├── platform-pc/     # wgpu + cpal + winit; perfis de simulação de console
│   ├── platform-n64/    # FFI libdragon (bindgen próprio, pinado)
│   ├── platform-3ds/    # FFI ctru-rs/citro2d/citro3d
│   ├── editor/          # egui: viewport, hierarquia, inspector, asset browser, play/stop
│   └── asset-pipeline/  # lib compartilhada por xtask e editor (bake + watch)
├── apps/{pc,n64,3ds}/   # binários de cola (main mínimo por plataforma)
├── examples/            # jogos-exemplo — também são testes de integração
├── assets/              # masters compartilhados + overrides por plataforma
│   ├── shared/
│   ├── n64/  3ds/  pc/  # overrides; manifest.toml por asset (formatos CI4/RGBA5551/…)
├── platforms/{n64,3ds,pc}.toml
├── xtask/               # assets|build|run|test|watch|new <plataforma>
├── templates/new-game/  # scaffold `trino new` — inclui AGENTS.md + .claude/skills/
├── tests/               # ROMs/apps de teste por feature + golden images
├── .github/
│   ├── workflows/{ci.yml, release.yml}
│   └── ISSUE_TEMPLATE/  # forms YAML (dropdown plataforma, emulador-vs-hardware)
├── .claude/skills/      # build-n64, build-3ds, run-emulator, test-all, add-asset, new-material, release
├── AGENTS.md  CLAUDE.md  README.md  CONTRIBUTING.md  CODE_OF_CONDUCT.md
├── LICENSE-MIT  LICENSE-APACHE
└── docs/
```

---

## 4. Fases

Cada fase lista: **objetivo → tarefas ordenadas → testes automatizados novos → critérios
de aceite**. Nenhuma fase fecha sem seus testes rodando no CI.

---

### Fase 0 — Andaime + infraestrutura (a base existe desde o commit 1)

**Objetivo:** workspace compilando, CI verde, repo já AI-friendly e com cara de open source.

**Tarefas:**
1. `cargo new` workspace com `crates/core`, `crates/game-api`, `xtask`; resolver = "2".
2. `core`: traits vazios porém **assinados** — `Renderer`, `Audio`, `Input`, `Platform`,
   tipos `SpriteId`, `ModelId`, `Material` (enum `Sprite | VertexLit | Named(MaterialId)`),
   `Caps` (limites N64). `#![no_std]`, zero dependências.
3. `xtask` esqueleto: subcomandos `build|run|test|assets|watch|new <plataforma>` (só PC
   funcional nesta fase; os demais imprimem "fase futura").
4. **AGENTS.md v1**: mapa do repo, direção de dependências, comandos xtask, armadilhas.
   **CLAUDE.md** com primeira linha `@AGENTS.md` (import — não symlink; symlink no Windows
   exige admin).
5. `.claude/skills/` iniciais: `test-all`, `add-asset` (esqueleto). Formato:
   `SKILL.md` com frontmatter `description:`; skill `release` com
   `disable-model-invocation: true`; <500 linhas cada; scripts em `scripts/`.
6. Higiene OSS: LICENSE-MIT + LICENSE-APACHE, CODE_OF_CONDUCT (Contributor Covenant),
   CONTRIBUTING.md (esqueleto), issue forms YAML, PR template.
7. README v1: badges (CI, licença), descrição de uma linha, quickstart `git clone` →
   `cargo xtask run pc` (janela vazia), matriz de suporte com status honesto (🚧).
8. **ci.yml v1**: job desktop na matriz `ubuntu-24.04 / windows-2025 / macos-latest` com
   `dtolnay/rust-toolchain` + `Swatinem/rust-cache@v2`: `cargo fmt --check`,
   `cargo clippy -D warnings`, `cargo test --workspace`.
9. Teste de arquitetura: teste que parseia `cargo metadata` e **falha se** `core` ganhar
   dependência, ou `game`/`platform-*` dependerem de algo além de `core`.

**Testes novos:** unit dos tipos de `core` (math, `Caps`), teste do grafo de dependências.

**Aceite:** CI verde nos 3 OS; clone → `cargo xtask run pc` abre janela vazia; Claude Code
aberto no repo entende o projeto só pelo AGENTS.md.

---

### Fase 1 — PC 2D + traits core de verdade

**Objetivo:** `platform-pc` implementa os traits; um jogo mínimo roda: sprites, input, som.

**Tarefas:**
1. `platform-pc`: winit (janela/loop), wgpu (device/surface), renderer 2D de sprites
   (batch, atlas), cpal (áudio), input (teclado/gamepad via gilrs).
2. `crates/game` de exemplo: usa **apenas** `core` — move um sprite com input, toca som.
3. `apps/pc`: cola `platform-pc` + `game`.
4. Render offscreen: caminho de render para textura (base do golden test **e** do viewport
   do editor na Fase 3 — mesma infraestrutura, escrever uma vez).
5. Fundações da simulação de console: framebuffer interno com resolução configurável
   (320×240 / 400×240 / nativa) + upscale nearest inteiro.

**Testes novos:**
- Unit: batcher de sprites, transformações, mixer de áudio (buffers sintéticos).
- **Golden image**: renderiza cena de teste offscreen (Mesa software em CI, xvfb), compara
  PNG com tolerância; regenerar goldens via `cargo xtask test --bless`.
- Smoke: app abre, roda 60 frames, fecha com exit 0 (CI: `xvfb-run -a`).

**Aceite:** `cargo xtask run pc` roda o jogo-exemplo; golden tests no CI Linux; jogo não
importa nada além de `core`.

---

### Fase 2 — Pipeline de assets + live reload no PC

**Objetivo:** assets com masters compartilhados + overrides por plataforma; mudou arquivo →
jogo atualiza sem reiniciar; mudou código do jogo → dylib recarrega.

**Tarefas:**
1. `asset-pipeline`: parse de `manifest.toml` por asset (formato por plataforma:
   `CI4`, `RGBA5551`, `RGBA8`…), resolução híbrida (override > shared; shader/combiner
   ausente para o alvo = **erro de build**, não fallback).
2. Bake PC: imagens → atlas/texturas, áudio → PCM. Saída em `target/assets/pc/`.
3. Handles estáveis: asset ID = hash do caminho lógico; recarga troca conteúdo in-place,
   handles nunca invalidam.
4. Watch: notify 8.x + notify-debouncer-full (debounce 50–200 ms), rebake incremental →
   canal para o runtime → swap.
5. Hot reload de código: `game-api` define a fronteira `repr(C)` (`GameState` opaco do
   lado do host + fns `init/update/render/on_reload`); `game` vira
   `crate-type=["rlib","dylib"]`; host usa hot-lib-reloader 0.8.2 com `LibReloadObserver`
   para serializar/migrar estado entre versões (serde). Feature `reload` (default no dev
   PC; consoles/release = estático).
6. Profile `dev-hot-reload` + `RUSTFLAGS=-C prefer-dynamic` encapsulados no
   `cargo xtask watch pc`.
7. Documentar armadilhas do hot reload no AGENTS.md (UB por drift de layout, sem generics,
   statics/TypeId resetam, tracing incompatível).

**Testes novos:**
- Snapshot do pipeline: manifest de teste → bake → compara metadados/hashes de saída.
- Teste de resolução híbrida (override vence; ausência = erro).
- Teste de recarga de asset: bake v1 → carrega → bake v2 → verifica swap sem invalidar handle.
- Teste E2E de hot reload (Linux CI): compila dylib v1, roda host, recompila v2 com
  comportamento diferente, asserta que estado sobreviveu e comportamento mudou.

**Aceite:** editar PNG → sprite atualiza <1 s sem restart; editar `game/src/lib.rs` →
recompila e recarrega mantendo posição do jogador; tudo coberto no CI.

---

### Fase 3 — Editor v1

**Objetivo:** shell visual estilo Unity: viewport, hierarquia de cena, inspector, asset
browser, play/stop. **Formato de cena definido AQUI e congelado** (lição do Bevy: cedo,
texto, versionado).

**Tarefas:**
1. Formato de cena: RON ou TOML versionado (`version = 1`), entidades + componentes
   serde. Tudo downstream depende disso — definir primeiro.
2. Shell eframe + egui 0.35 + egui_dock 0.20.1: layout com painéis Viewport / Hierarquia /
   Inspector / Assets / Console.
3. Viewport: render da cena para textura wgpu →
   `register_native_texture(..., FilterMode::Nearest)` → `ui.image`; **recriar e
   re-registrar no resize** (senão vaza memória GPU). Câmera de editor (pan/zoom 2D).
4. Inspector: reflexão simples via derive próprio (`#[derive(Inspect)]`) para editar
   componentes; salvar/carregar cena.
5. Asset browser próprio: árvore de `assets/`, lista virtualizada (egui_extras), preview
   de sprite, drag-and-drop para a cena.
6. **Play mode**: processo separado spawnado via cargo (modelo Fyrox) — crash do jogo não
   derruba o editor; stop = kill. Live reload da Fase 2 funciona dentro do play mode.
7. Seletor de modo de simulação na toolbar: `PC | N64 look | 3DS look | PC-estrito`
   (resoluções por enquanto; o visual N64 completo chega na Fase 4).
8. Gizmo de transform: transform-gizmo-egui 0.9.0 — ⚠️ suporta só egui 0.34; decidir:
   pinar egui 0.34 no editor ou vendorar o gizmo (registrar decisão em docs/).
9. Perf: mimalloc, repaint-on-demand, puffin_egui atrás de feature `profile`.

**Testes novos:**
- Round-trip de cena: cena → salvar → carregar → igual (proptest com cenas geradas).
- Teste de migração de versão do formato de cena (v1 → v1, arnês pronto para v2).
- Smoke do editor em CI: abre com `xvfb-run`, carrega cena de exemplo, renderiza 10
  frames, fecha limpo.
- Golden do viewport: cena conhecida → screenshot do viewport → compara.

**Aceite:** `cargo xtask editor` abre; dá para montar uma cena de sprites, salvar, apertar
play, ver o jogo rodando, editar um asset e ver atualizar ao vivo.

---

### Fase 4 — Nintendo 64

**Objetivo:** a mesma cena/jogo roda em ROM `.z64` no ares e em hardware; testes N64 no
CI; modo "N64 look" real no PC/editor.

**Tarefas:**
1. Toolchain: target `mips-nintendo64-none.json` + `rust-toolchain.toml` pinando
   `nightly-2025-01-12`; runner **nust64 0.4.1** no `.cargo/config.toml`;
   `-Zbuild-std=core,alloc`; rustflags `-Clto -Cembed-bitcode`.
2. Bindings libdragon próprios: build.rs com bindgen contra headers do container
   `ghcr.io/dragonminded/libdragon@sha256:<digest pinado>`; wrapper seguro mínimo
   (display, rdpq, joypad, audio, dfs, debug/ISViewer). **Não** usar sarchar/libdragon-rs.
3. `xtask build n64`: roda o build dentro do Docker de forma transparente (Windows:
   Docker Desktop; CLI npm `libdragon` v12.2.1 como alternativa).
4. Assets N64 no pipeline: mksprite (`.sprite`, formatos CI4/RGBA5551 do manifest),
   audioconv64 (`.wav64`), mkfont, mkdfs → `.dfs` anexado à ROM.
5. `platform-n64` implementa os traits de `core`; `apps/n64` gera `.z64` do jogo-exemplo.
6. Canal de teste: ISViewer (`0xB3FF0020`) — `debugf` de magic strings
   `TRINO_TEST_PASS` / `TRINO_TEST_FAIL:<msg>` (padrão n64-systemtest).
7. Harness de emulador no xtask: `cargo xtask test n64` builda ROM de teste → lança
   **ares v148** (`--system "Nintendo 64" rom.z64 --no-file-prompt`, Homebrew Mode ativo,
   xvfb no CI) → captura stdout → timeout → kill → exit code pelas magic strings.
8. Golden images N64: **mupen64plus-ui-console**
   `--testshots 30,60 --sshotdir out --nospeedlimit` → compara PNGs (tolerância maior;
   documentar risco de RSP HLE com microcode rdpq — se divergir, marcar teste como
   "somente ares").
9. Live reload N64: `cargo xtask watch n64` = watch → rebuild ROM → relaunch ares
   (sem IPC de reload; kill+relaunch). Hardware: UNFLoader v2.2 `-r rom.z64 -l`
   (re-upload automático), `-d` debug via USB, `-g` GDB; SummerCart64 via `sc64deployer`.
10. **Modo N64 look no PC** (agora com referência real do emulador):
    - 3-point bilinear no shader de material (3 fetches, porta do Shadertoy `Ws2fWV`);
    - quantização RGBA5551 + dither ordenado (magic-square/Bayer, ref. GLideN64 #2183);
    - estágio VI opcional (dedither/divot/AA de borda, ref. parallel-rdp);
    - 320×240 offscreen + nearest inteiro.
    - Golden test PC-look vs. screenshot do ares da MESMA cena (tolerância frouxa —
      é aproximação, o emulador é a verdade).
11. Modo PC-estrito: `Caps` do N64 (TMEM 4 KB, limites de textura, contagem de tris)
    validados em debug — estourou, erro com mensagem acionável.
12. Skills novas: `build-n64`, `run-emulator`. CI: job N64 no container libdragon
    (digest pinado) buildando ROM + rodando testes ares sob xvfb.

**Testes novos:** ROM de teste por feature (init vídeo, sprite, input via mock, áudio,
DFS) reportando via ISViewer; golden mupen64plus; golden do look-mode; teste de `Caps`.

**Aceite:** `cargo xtask run n64` abre ares com o jogo; `cargo xtask test n64` passa
local e no CI; toggle "N64 look" no editor visivelmente próximo do emulador.

---

### Fase 5 — Nintendo 3DS

**Objetivo:** mesmo jogo em `.3dsx` no Azahar e em hardware; testes 3DS no CI; modo
"3DS look".

**Tarefas:**
1. Toolchain: nightly + `armv6k-nintendo-3ds` (Tier 3, build-std) + devkitARM
   (devkitPro pacman `3ds-dev`; Windows: instalador gráfico oficial) + cargo-3ds 0.1.5.
2. `platform-3ds`: ctru-rs (git dep — pinar rev) para gfx/hid/ndsp/romfs; **FFI próprio
   para citro2d** (não há bindings oficiais) para o 2D; assinatura igual aos outros
   platform-*.
3. Assets 3DS: bake para formatos nativos (tex3ds via pipeline), romfs.
4. `apps/3ds` → `.3dsx` do jogo-exemplo; rodar no **Azahar 2125.1.3** (GUI; launch via
   CLI é historicamente bugado — issue #2066; abrir via associação de arquivo/GUI local).
5. Testes em CI: **rust3ds/actions** `setup@v1` + `run-tests@v1` na imagem
   `devkitpro/devkitarm:20260610` (libctru/citro3d/citro2d já em `/opt/devkitpro`) —
   fork do Citra com GDB-stub → exit codes reais + `--dump-video` webm como artifact.
   Limite conhecido: testes que precisam de romfs não rodam nesse harness — cobrir romfs
   com teste local documentado.
6. Live reload 3DS: `cargo xtask watch 3ds` = rebuild → re-upload via **3dslink**
   (`cargo 3ds run --address <ip do console>`; `--server` streama stdout). No emulador:
   kill+relaunch do Azahar, ou core libretro no RetroArch (UDP 55355 aceita
   RESET/QUIT — reload scriptável).
7. **Modo 3DS look** no PC/editor: 400×240 (tela de cima), restrições fixed-function nos
   materiais, bilinear, 24-bit.
8. Skill `build-3ds`; job 3DS no ci.yml.

**Testes novos:** suite `cargo 3ds test` rodando no Citra do CI (unit no console),
golden do look-mode 3DS, teste do pipeline de assets 3DS.

**Aceite:** `cargo xtask run 3ds` produz `.3dsx` que roda no Azahar; CI 3DS verde;
mesma cena roda nos 3 alvos a partir do mesmo código de jogo.

---

### Fase 6 — Jogo de plataforma completo (prova da engine)

**Objetivo:** um jogo real de plataforma 2D — tilemap, colisão, câmera, cenas, áudio,
HUD — rodando **idêntico** nos 3 alvos. Vira o exemplo-vitrine do README.

**Tarefas:**
1. Módulos de gameplay em `core`/crates auxiliares (sempre plataforma-agnósticos):
   tilemap (bake por plataforma: atlas + colisão), AABB/colisão, câmera com bounds,
   máquina de cenas, spawn de entidades a partir do formato de cena do editor.
2. Áudio música + SFX nos 3 alvos (xm64 no N64, ndsp no 3DS, cpal no PC).
3. Editor: pintar tilemap no viewport, colocar entidades, definir spawn.
4. Polir o loop completo: editar no editor → play PC → `watch n64`/`watch 3ds` →
   hardware.
5. Gravar o GIF hero do README (editor + mesma cena nos 3 consoles).

**Testes novos:** unit de colisão/física (proptest), golden da mesma cena nos 3 alvos
(PC offscreen, mupen64plus testshots, Citra dump-video frame), teste de determinismo do
update (mesma seed/inputs → mesmo estado), smoke do jogo completo por plataforma no CI.

**Aceite:** jogo jogável do início ao fim nos 3 alvos; qualquer feature usada pelo jogo
tem teste; GIF no README.

---

### Fase 7 — 3D

**Objetivo:** `draw_model` nos 3 alvos com materiais preset.

**Tarefas:**
1. `core`: `ModelId`, `Material::VertexLit`/`Named`, transform 3D, câmera perspectiva.
2. Pipeline: glTF master → `mkmodel` → `.model64` (N64, exige container `:preview` —
   pinar digest específico), → formato citro3d (3DS), → buffers wgpu (PC).
3. N64: tiny3d (branch preview do libdragon) via FFI próprio.
4. 3DS: citro3d-rs (`include_shader!` compila picasso em build time).
5. PC: pipeline WGSL vertex-lit; N64-look 3D (3-point bilinear já existe do material 2D).
6. Editor: câmera 3D no viewport, gizmo de transform (decisão da Fase 3 sobre versão).

**Testes novos:** golden 3D nos 3 alvos, teste de conversão glTF (vértices/índices/
materiais), `Caps` 3D (limite de tris/textura no modo estrito).

**Aceite:** cena 3D vertex-lit idêntica (dentro da tolerância) nos 3 alvos; editável no
editor.

---

### Fase 8 — Release 1.0 + template + polish

**Objetivo:** projeto instalável por terceiros; release automática; docs completas.

**Tarefas:**
1. **`trino new <nome>`** (via `cargo xtask new` e binário `trino`): scaffolda jogo a
   partir de `templates/new-game/` — inclui `AGENTS.md`, `CLAUDE.md` (`@AGENTS.md`),
   `.claude/skills/` (build/run/test por plataforma), cena inicial, CI próprio do jogo.
   *(Requisito explícito: todo projeto criado nasce AI-friendly.)*
2. **release.yml**: no push para `main`, lê versão do workspace; se não existe tag → cria
   tag + `softprops/action-gh-release@v3` com `generate_release_notes: true`. Artifacts:
   editor (Linux/Windows/macOS), demo `.z64`, demo `.3dsx`, `SHA256SUMS`.
   Caminho de upgrade documentado: `release-plz` (crates.io) e `cargo-dist` (instaladores).
3. README final: badge row (CI, release, licença, discord), hero GIF, pitch de 3
   parágrafos, quickstart <5 comandos, matriz de plataformas (features × alvo × status),
   seção "IA: abra este repo no Claude Code e peça o que quiser", links docs/CONTRIBUTING.
4. AGENTS.md completo + AGENTS.md aninhados por crate (quando o repo crescer);
   skills finais (`release` com `disable-model-invocation: true`).
5. CONTRIBUTING.md: setup por plataforma (Docker/devkitPro/Rust nightly), como rodar
   cada suite, convenções, como adicionar uma plataforma nova.
6. docs/: arquitetura (diagrama de crates), guia do editor, guia de assets, guia de
   hardware real (UNFLoader/SummerCart64/3dslink), decisões (ADRs curtos).

**Testes novos:** E2E do template (`trino new foo && cd foo && cargo xtask test pc` no
CI), dry-run do release.yml em branch, link checker no README/docs.

**Aceite:** merge na main gera release com todos os artifacts; `trino new` produz jogo
que builda e testa; README permite onboarding sem ajuda externa.

---

## 5. CI — especificação dos workflows

### ci.yml (push + PR)
| Job | Onde | O quê |
|---|---|---|
| `lint` | ubuntu-24.04 | fmt, clippy `-D warnings`, teste do grafo de deps |
| `desktop` | matriz ubuntu-24.04 / windows-2025 / macos-latest | `cargo test --workspace`, golden PC (Linux: xvfb + Mesa), build editor |
| `n64` | ubuntu-24.04, container `ghcr.io/dragonminded/libdragon@<digest>` | build ROMs, testes ares (xvfb) via ISViewer, golden mupen64plus |
| `3ds` | ubuntu-24.04, container `devkitpro/devkitarm:20260610` | rust3ds/actions `setup@v1` + `run-tests@v1`, artifact webm |
| `hot-reload` | ubuntu-24.04 | E2E dylib reload |
| `template` | ubuntu-24.04 | `trino new` + build do jogo gerado (a partir da Fase 8) |

Todos com `Swatinem/rust-cache@v2` (chave por target). Golden images ficam em
`tests/golden/`; regeneração só via `--bless` local + review no PR.

### release.yml (push na main)
1. Job `tag`: compara versão do workspace com tags existentes; nova → cria `vX.Y.Z`.
2. Job `build-artifacts` (needs tag, matriz 3 OS + jobs de console): editor por OS,
   `demo.z64`, `demo.3dsx`, checksums.
3. Job `release`: `softprops/action-gh-release@v3`, `generate_release_notes: true`,
   anexa artifacts.

---

## 6. Verificação end-to-end (após Fase 8)

1. Máquina limpa (Windows): clone → README → conseguir rodar PC em <10 min (Docker p/ N64
   documentado como opcional no primeiro contato).
2. `trino new meujogo` → editor abre → colocar sprite → play → editar PNG → hot reload →
   `cargo xtask run n64` → mesmo resultado no ares.
3. Abrir o repo no Claude Code sem contexto extra e pedir "adicione um material novo" —
   AGENTS.md + skills devem bastar.
4. Merge de PR dummy na main → release aparece com todos os artifacts → baixar editor e
   demo.z64 → rodar.
5. (Com hardware) UNFLoader `-r -l` no N64 real; 3dslink no 3DS real.

## 7. Riscos principais e mitigação

| Risco | Mitigação |
|---|---|
| ctru-rs/citro3d-rs só via git | Pinar rev; smoke de upgrade semanal opcional no CI (job allow-fail) |
| transform-gizmo-egui trava egui em 0.34 | Vendorar o gizmo OU segurar egui; ADR na Fase 3 |
| RSP HLE do mupen64plus renderiza rdpq errado | ares é a verdade; goldens divergentes marcados "ares-only" |
| Container `:preview` (tiny3d) instável | Pinar digest; só a Fase 7 depende dele |
| hot-lib-reloader UB em drift de layout | Fronteira `repr(C)` mínima em `game-api`; estado serializado na recarga; documentado no AGENTS.md |
| Azahar sem headless | CI usa fork Citra do rust3ds; Azahar é só workflow local |
| Runners GitHub migrando em 2026 | Labels pinadas (`ubuntu-24.04`, `windows-2025`) |
| Projeto ambicioso demais | Cada fase termina com demo funcional + CI verde; ordem foi desenhada p/ valor incremental |
