# Evidências do PR das issues #72, #99 e #110

## #72: auditoria de layout instável sob carga

- `logs/stress-summary.txt`: cada teste repetido 10 vezes com 16 `yes > /dev/null` ocupando a CPU.
  - Antes (`main`): `editor_layout_is_clean…` falhou 2 de 10 vezes e `particle_effects…` 1 de 10.
  - Depois: 10 de 10 nos dois testes e no teste novo dos toasts.
  - A linha `after particle…: fail=10` é uma etapa intermediária. Com o tempo do egui, cada passo do kittest avança 0,1 s; o teste passou a dar às faíscas uma vida longa (`after2`: 10 de 10).
- `logs/stress-before-play-here-failure.log`: uma falha da `main`. Os rótulos "fantasmas" em y 73, 122 e 171 aparecem no fim da árvore.
- `editor/toasts-first-frame-*.png`: o quadro em que o toast nasce.
  - **Nada aparece na tela**, porque é o passo invisível de dimensionamento do egui.
  - Os rótulos, porém, já estão na árvore de acessibilidade, nas mesmas posições das falhas: em 960×600, ícone em x 375–391 e y 73/122/171; em 1600×1000, texto em y 571–587, a linha da falha original do CI.
- `editor/toasts-settled-*.png`: o quadro que a auditoria julga agora.

## #99: atlas por idioma a 100 % da TMEM

`atlases/before-*` e `atlases/after-*` mostram os atlas I4 do N64 ampliados 4×.

| atlas | antes | depois | TMEM antes | TMEM depois |
|---|---|---|---|---|
| fonts/i18n (en-US) | 256×16 | 64×44 | 2048 B (50 %) | 1408 B (34 %) |
| fonts/i18n@es-ES | 256×32 | 16×188 | 4096 B (100 %) | 1504 B (37 %) |
| fonts/i18n@pt-BR | 256×32 | 32×100 | 4096 B (100 %) | 1600 B (39 %) |

## #110: heap do N64 base

- `logs/n64-before.txt`: a `main`.
- `logs/n64-final.txt`: este PR, nos dois tiers.
- `logs/n64-arena-0.txt` e `logs/n64-arena-416.txt`: o A/B que decidiu que a arena é opt-in. Com 416 KB, `memory allocation of 79200 bytes failed`.
- `logs/n64-paddings.md`: os tempos de quadro, em média (mín–máx) sobre 4 paddings de código, com o Expansion Pak.
