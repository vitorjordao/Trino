# Trino — Engine Rust multiplataforma (N64 / 3DS / PC)

> Nome de trabalho: **trino** (três alvos). Renomeie à vontade — é só trocar
> os nomes dos crates no `Cargo.toml`.

Plano de arquitetura para uma engine de jogo 2D/3D que roda em **Nintendo 64**,
**Nintendo 3DS** e **PC**, escrita em Rust, com pipeline de assets que recebe
um master por tipo (glTF, PNG, WAV) e gera os formatos nativos de cada console
no momento do build.

---

## 1. Princípios que guiam todo o design

1. **Não é uma engine genérica — é a abstração mais fina que o *seu* jogo precisa.**
   A API nasce pequena e cresce só quando um jogo real exige.
2. **O N64 é o teto de todas as decisões.** Pipeline fixo, sem fragment shader,
   cache de textura de 4 KB, 4–8 MB de RAM. Se cabe no N64, cabe trivialmente no
   resto. O caminho inverso obriga a refazer arte e código.
3. **O backend de PC valida as restrições do N64.** Em modo debug ele recusa
   texturas grandes demais, materiais fora dos presets, etc. "Se passou no PC,
   roda no N64." Itera-se rápido no PC e valida-se no emulador e no hardware.
4. **Compartilhe o que é portável; isole o que é intrínseco à máquina.**
   Lógica e assets-fonte são compartilhados; shaders/combiners e formatos baked
   são específicos por plataforma.

### O que é compartilhado vs. específico

| Categoria | Compartilhado (`shared/`, `core`, `game`) | Específico por plataforma |
|---|---|---|
| Lógica do jogo | ✅ Rust `no_std` no crate `game` | — |
| Math, ECS-lite, API de render | ✅ crate `core` (traits) | impls nos `platform-*` |
| Modelos 3D | ✅ master glTF/OBJ | formato baked (model64, vertex buffer) |
| Sprites/texturas | ✅ master PNG | baked (sprite N64, .t3x 3DS) + escolha de formato |
| Áudio | ✅ master WAV + XM | baked (wav64, PCM16/DSP-ADPCM) |
| Níveis/tilemaps | ✅ LDtk/Tiled | baked binário |
| **Shaders / combiners** | ❌ (intrínsecos) | **.wgsl (PC) / .pica (3DS) / combiner.toml (N64)** |
| Resolução, budget de memória, formato de textura padrão | ❌ | `platforms/<console>.toml` |
| Entry point, linker, IPL3, runner | ❌ | `.cargo/config.toml` + crate `apps/<console>` |

---

## 2. Estrutura de pastas completa

```
trino/
├── Cargo.toml                  # workspace
├── rust-toolchain.toml         # nightly + rust-src (build-std p/ N64)
├── .cargo/
│   └── config.toml             # targets, runners, build-std por plataforma
│
├── crates/
│   ├── core/                   # no_std + alloc — o coração portável
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── math.rs         # vec/mat/quat fixos p/ N64 (cuidado com float)
│   │       ├── render.rs       # trait Renderer (desenhada p/ o teto do N64)
│   │       ├── audio.rs        # trait Audio
│   │       ├── input.rs        # trait Input + mapeamento de botões abstrato
│   │       ├── platform.rs     # trait Platform (agrega render+audio+input+time)
│   │       ├── assets.rs       # tipos baked + handles (Sprite, Model, Sound)
│   │       └── material.rs     # enum/ids de materiais (presets, não shaders)
│   │
│   ├── game/                   # no_std — SÓ regras do jogo, não conhece plataforma
│   │   └── src/
│   │       ├── lib.rs          # pub fn run(p: &mut impl Platform)
│   │       ├── player.rs
│   │       ├── physics.rs
│   │       └── scenes/
│   │
│   ├── platform-pc/            # backend desktop
│   │   └── src/
│   │       ├── lib.rs          # pub fn boot(f: impl FnOnce(&mut PcCtx))
│   │       ├── render_wgpu.rs  # impl Renderer via wgpu/macroquad
│   │       ├── audio.rs
│   │       └── validate.rs     # checa restrições do N64 em debug
│   │
│   ├── platform-3ds/           # backend 3DS (FFI p/ citro3d/citro2d via ctru-rs)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── render_citro.rs
│   │       └── audio_ndsp.rs
│   │
│   └── platform-n64/           # backend N64 (FFI p/ libdragon)
│       └── src/
│           ├── lib.rs
│           ├── render_rdp.rs   # display lists + combiner modes
│           ├── audio_mixer.rs
│           └── ffi/            # bindings libdragon (bindgen ou à mão)
│
├── apps/                       # binários finos de "cola" — ~10 linhas cada
│   ├── pc/   (bin)             # main: platform_pc::boot(|c| game::run(c))
│   ├── 3ds/  (bin)
│   └── n64/  (bin)
│
├── assets/                     # ===== MASTERS (você edita aqui) =====
│   ├── shared/                 # portável — convertido p/ cada plataforma
│   │   ├── models/level1.glb
│   │   ├── sprites/player.png
│   │   ├── sprites/tiles.png
│   │   ├── audio/jump.wav
│   │   ├── audio/music.xm
│   │   ├── data/level1.ldtk
│   │   └── materials.toml      # definição ABSTRATA dos materiais
│   ├── pc/
│   │   └── shaders/
│   │       ├── sprite_basic.wgsl
│   │       └── water.wgsl
│   ├── 3ds/
│   │   └── shaders/
│   │       ├── sprite_basic.v.pica   # vertex shader (montado por picasso)
│   │       └── water.v.pica
│   ├── n64/
│   │   └── combiners/
│   │       ├── sprite_basic.toml     # config de combiner mode do RDP
│   │       └── water.toml            # (N64 não tem shader programável)
│   └── manifest.toml           # como bakear cada asset + overrides por plataforma
│
├── build/                      # ===== GERADO (gitignore) =====
│   ├── pc/
│   ├── 3ds/
│   └── n64/
│
├── platforms/                  # config individual de cada console
│   ├── pc.toml
│   ├── 3ds.toml
│   └── n64.toml
│
├── tools/                      # wrappers dos conversores externos
│   ├── mksprite, audioconv64   # (libdragon)  — N64
│   ├── tex3ds, picasso         # (devkitPro)  — 3DS
│   └── README.md               # como instalar cada toolchain
│
└── xtask/                      # orquestrador de build (o "cargo xtask")
    └── src/
        ├── main.rs             # cargo xtask build|run|assets <plataforma>
        ├── assets.rs           # roda o pipeline lendo manifest.toml
        └── resolve.rs          # regra de resolução híbrida (shared + override)
```

---

## 3. As camadas e a direção das dependências

```
        core  (traits + math, no_std, ZERO deps)
        ▲   ▲
        │   └────────────┐
      game             platform-pc / platform-3ds / platform-n64
   (no_std, só          (cada um impl as traits do core)
    conhece core)              ▲
        ▲                      │
        └──────── apps/<console> ────────┘
              (cola game + 1 platform)
```

- `core` não conhece ninguém.
- `game` depende **só** de `core` — nunca sabe em que máquina roda.
- cada `platform-*` depende **só** de `core` — nunca conhece o jogo.
- `apps/<console>` é o único lugar que junta os dois. ~10 linhas:

```rust
// apps/pc/src/main.rs
fn main() {
    platform_pc::boot(|ctx| game::run(ctx));
}
```

```rust
// apps/n64/src/main.rs
#![no_std]
#![no_main]
#[no_mangle]
pub extern "C" fn main() -> ! {
    platform_n64::boot(|ctx| game::run(ctx))
}
```

Trocar de jogo = trocar a dependência do `apps/*`. Portar pra um 4º console
(PSP? Switch?) = adicionar um `platform-psp` e um `apps/psp`, sem tocar em
`core` nem `game`.

---

## 4. A trait `Renderer` mínima (desenhada pro teto do N64)

A regra: nada de fragment shader arbitrário na API. Materiais são um **enum de
presets**; cada plataforma resolve esse preset pro seu programa real.

```rust
// crates/core/src/render.rs   (no_std)
pub struct SpriteId(pub u16);
pub struct ModelId(pub u16);

#[derive(Clone, Copy)]
pub enum Material {
    /// blit de sprite com alpha (mapeia em todos)
    Sprite,
    /// malha com cor por vértice (Gouraud) — denominador comum 3D
    VertexLit,
    /// material nomeado e resolvido por plataforma (ex.: "water")
    Named(MaterialId),
}

pub trait Renderer {
    fn begin_frame(&mut self);
    fn clear(&mut self, color: Rgba);

    // 2D
    fn draw_sprite(&mut self, s: SpriteId, x: i32, y: i32);
    fn draw_sprite_ex(&mut self, s: SpriteId, dst: Rect, src: Rect, tint: Rgba);

    // 3D (transform + material-preset, sem shader livre)
    fn draw_model(&mut self, m: ModelId, transform: &Mat4, mat: Material);
    fn set_camera(&mut self, view: &Mat4, proj: &Mat4);

    fn end_frame(&mut self);

    // budget — o backend de PC usa isto p/ validar contra o N64
    fn caps(&self) -> Caps;
}

pub struct Caps {
    pub max_texture_px: u16,   // N64: pequeno; PC: enorme
    pub max_tris_frame: u32,
    pub has_free_shaders: bool,
}
```

`Material::Named` é o que liga o sistema de shaders híbrido (próxima seção).

---

## 5. O esquema híbrido de assets (shared + override)

### Regra de resolução

No build para a plataforma `P`, para cada asset lógico (ex.: material `water`):

1. procura `assets/<P>/.../water.*` (versão específica);
2. se não achar **e o asset for portável** (PNG, glTF, WAV) → usa `assets/shared/...`;
3. se não achar **e for intrínseco** (shader/combiner) → **erro de build**.

O passo 3 é proposital: shader não tem fallback. Faltar o `.pica` do 3DS ou o
combiner do N64 *deve* quebrar o build, te obrigando a fornecer os três.

### `assets/shared/materials.toml` — definição abstrata

```toml
[material.sprite_basic]
kind    = "sprite"
blend   = "alpha"

[material.water]
kind    = "model"
blend   = "alpha"
texture = "water_tex"     # nome lógico; cada plataforma bakeia do seu jeito
scroll  = true            # propriedade que CADA backend interpreta como puder
```

O mesmo material `water` é implementado por arquivos de mesmo nome:

```
assets/pc/shaders/water.wgsl        # shader real (fragment programável)
assets/3ds/shaders/water.v.pica     # vertex shader PICA200 (montado por picasso)
assets/n64/combiners/water.toml      # combiner mode do RDP (sem shader)
```

Exemplo do combiner N64 (config declarativa, não código):

```toml
# assets/n64/combiners/water.toml
# Combiner do RDP: como misturar textura, cor de vértice e alpha.
cycle      = "1cycle"
color_a    = "TEX0"
color_b    = "SHADE"
blend      = "alpha"
# "scroll" do material vira deslocamento de coordenada de textura por frame,
# feito na CPU/RSP antes de montar a display list.
```

### `assets/manifest.toml` — pipeline + overrides de formato

```toml
[texture.player]
source       = "shared/sprites/player.png"
n64.format   = "CI4"        # palettizado, 16 cores — cabe nos 4 KB de TMEM
3ds.format   = "RGBA5551"
pc.format    = "RGBA8"

[texture.tiles]
source       = "shared/sprites/tiles.png"
n64.format   = "CI8"
3ds.format   = "RGB565"
pc.format    = "RGBA8"

[model.level1]
source       = "shared/models/level1.glb"
n64.lod      = "low"        # mesmo asset, malha decimada só no N64
3ds.lod      = "med"
pc.lod       = "high"

[sound.jump]
source       = "shared/audio/jump.wav"
# todos viram o formato nativo: wav64 (N64), PCM16 (3DS), WAV/OGG (PC)

[music.theme]
source       = "shared/audio/music.xm"
n64.driver   = "xm64"       # libdragon toca XM nativo
3ds.driver   = "pcm_stream" # decodifica e faz streaming
pc.driver    = "any"
```

Repare: **um master por asset**, e o `manifest.toml` diz como cada console o
adapta. É exatamente a ideia que você descreveu — subir um arquivo, gerar todos.

---

## 6. Configuração individual por console

### `platforms/n64.toml`

```toml
target            = "mips-nintendo64-none"
screen            = { w = 320, h = 240 }
mem_budget_mb     = 4          # 8 com Expansion Pak
max_texture_px    = 64         # por tile (limite de TMEM)
default_tex_format = "CI4"
audio_driver      = "libdragon_mixer"
output            = "rom"      # via nust64 + IPL3 do libdragon
emulator          = "ares"
```

### `platforms/3ds.toml`

```toml
target            = "armv6k-nintendo-3ds"
screen            = { w = 400, h = 240 }   # tela de cima
max_texture_px    = 1024
default_tex_format = "RGBA5551"
audio_driver      = "ndsp"
output            = "3dsx"
emulator          = "azahar"   # sucessor do Citra
```

### `platforms/pc.toml`

```toml
target            = "x86_64-unknown-linux-gnu"  # (ajuste p/ seu SO)
screen            = { w = 1280, h = 720 }
max_texture_px    = 8192
default_tex_format = "RGBA8"
audio_driver      = "cpal"
output            = "executable"
strict_n64_caps   = true       # valida budget do N64 em debug
```

### `rust-toolchain.toml`

```toml
[toolchain]
channel    = "nightly"
components = ["rust-src"]       # necessário p/ build-std (N64 no_std)
```

### `.cargo/config.toml`

```toml
[target.mips-nintendo64-none]
runner = ["nust64", "--libdragon", "release", "--elf"]

[target.armv6k-nintendo-3ds]
# 3DS é construído via `cargo 3ds build/run` (subcomando do cargo-3ds),
# então o xtask chama esse subcomando em vez de um runner.

[unstable]
build-std = ["core", "alloc"]   # aplicado só aos alvos bare-metal
```

---

## 7. O pipeline de assets (`xtask`)

Tudo passa por um único comando, pra você nunca precisar lembrar os detalhes de
cada toolchain:

```
cargo xtask assets n64     # só bakeia assets do N64 em build/n64/
cargo xtask build  3ds     # bakeia assets + compila o app do 3DS
cargo xtask run    pc      # bakeia + compila + roda no PC
cargo xtask run    n64     # bakeia + compila ROM + abre no ares
cargo xtask build  all     # os três
```

O que o `xtask assets <P>` faz, lendo `manifest.toml` e `platforms/<P>.toml`:

1. para cada asset, resolve o arquivo (regra híbrida da seção 5);
2. chama o conversor certo:
   - **N64** → `mksprite` / `audioconv64` / mkmodel do libdragon;
   - **3DS** → `tex3ds` (texturas) / `picasso` (shaders .pica);
   - **PC** → copia o master (carregado em runtime);
3. escreve em `build/<P>/` + gera um índice (`assets.rs` ou um arquivo binário
   de manifesto) que o `core::assets` carrega com os `SpriteId`/`ModelId`.

Assim os IDs de asset são os mesmos em todas as plataformas — o `game` referencia
`SpriteId(PLAYER)` e cada backend sabe onde achar o baked correspondente.

---

## 8. Loop de build e teste

```
edita game (Rust) ──► cargo xtask run pc        (iteração rápida, segundos)
        │                    │
        │              valida caps do N64 em debug
        ▼
edita arte/áudio ──► cargo xtask run 3ds  (Azahar)
        │
        ▼
        marco ──────► cargo xtask run n64  (ares)  ──► hardware real (flashcart)
```

- **PC**: 95% do desenvolvimento. Loop mais rápido + validação de restrições.
- **3DS**: Azahar (fork ativo do Citra) p/ iterar; `cargo 3ds` roda direto no
  console pela rede pelo Homebrew Launcher.
- **N64**: ares (preciso) ou cen64; depois flashcart (EverDrive) no hardware.

---

## 9. Roadmap em fases

**Fase 0 — Andaime.** Workspace, `core` com traits vazias, `platform-pc` que
abre uma janela e limpa a tela, `apps/pc` rodando. Nada de N64/3DS ainda.

**Fase 1 — 2D no PC.** `draw_sprite`, carregar PNG, input. Um quadrado que se
move. O `xtask assets pc` já funcionando (mesmo que só copiando PNG).

**Fase 2 — Pipeline real + N64 2D.** Implementa `platform-n64` (FFI libdragon),
`mksprite` no xtask, formato CI4. Mesmo jogo 2D rodando no ares. **Aqui mora a
maior parte do esforço** — assuma isso.

**Fase 3 — 3DS 2D.** `platform-3ds` via citro2d, `tex3ds` no xtask. Mesmo jogo
nos três alvos.

**Fase 4 — Plataforma jogável.** Tilemap (LDtk), colisão, áudio (WAV+XM nos três),
cenas. Esse é o "jogo de plataforma" do objetivo original.

**Fase 5 — 3D (opcional).** `draw_model` + `Material::VertexLit`, glTF→model64,
o sistema de materiais nomeados/shaders híbridos. Só depois que o 2D estiver
sólido nos três.

---

### Resumo da viabilidade

- **Pipeline de assets (subir 1 master → gerar tudo):** tranquilo, é trabalho
  conhecido.
- **Abstração 2D:** muito factível, mapeia limpo nos três.
- **Abstração 3D genérica:** ambiciosa mas possível, desde que projetada pro
  teto do N64 e crescida só quando o jogo exigir.
- **Esforço:** o backend do N64 (FFI libdragon + RDP) consome a maior fatia do
  tempo. PC e 3DS são bem mais mansos.
