# Evidence for PR #104 (issue #62, unified profiler)

- `perfetto/`: real traces opened in ui.perfetto.dev — `n64-hub` (ares, `test n64 --trace`), `3ds-hub` (Azahar, `test 3ds --trace`), `pc-castle64` (`run pc --trace`, cpu + gpu tracks).
- `editor/`: the Profiler tab (N64 timeline + frame flame + zones; `n64-hub` vs `n64-cpu-hub` diff at 1600×940 and 960×600; the PC trace) and the Inspector's measured N64 line.
- `tracy/`: `tracy-csvexport` of a 600-frame castle64 run captured by `tracy-capture` 0.14.1 (`cpu.csv` zones, `gpu.csv` GPU zones per frame).
- `data/`: the traces themselves (open them in Perfetto).
