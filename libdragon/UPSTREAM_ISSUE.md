eeprom_write_bytes corrupts the rest of a block when a short write starts on a block boundary

On `preview`, `eeprom_write_bytes` overwrites the rest of a block on EEPROM when the write starts on a block boundary and ends inside the same block. This only happens if the block is not in the eeprom cache yet. The bytes after the span get whatever the cache memory held. The cache comes from `malloc` (in my ares run it held zeros).

Repro (any ROM with `N64_ROM_SAVETYPE=eeprom4k`, on a block nothing has read or written since boot):

```c
uint8_t data[3] = { 0x10, 0x11, 0x12 };
eeprom_write_bytes(data, 136, 3);   // block 17, bytes 0..2
eeprom_wait_idle();
```

```
block 17      0    1    2    3    4    5    6    7
before       88   89   8a   8b   8c   8d   8e   8f
expected     10   11   12   8b   8c   8d   8e   8f
actual       10   11   12   00   00   00   00   00
```

Expected: only bytes 0..2 of block 17 change. Actual: bytes 3..7 are lost too.

Affected: `preview` at 39d0d6096 (the write-back cache from #843). Reproduced in ares v148 with a 4 Kbit EEPROM.

Cause: `eeprom_write_bytes` reads a block back before a partial write only in two cases:

https://github.com/DragonMinded/libdragon/blob/39d0d6096130836a65710da6730290701d060272/src/joybus/eeprom.c#L384-L390

```c
bool need_rmw_prefix = first_offset || (last_block != first_block && end_offset);
...
    if (first_offset)
        eeprom_cache_ensure_block_valid(first_block);
    if (last_block != first_block && end_offset)
        eeprom_cache_ensure_block_valid(last_block);
```

With `start = 136, len = 3`, `first_offset` is 0 and `last_block == first_block`, so neither case applies. The block is then marked valid and dirty (L396-L401), and the flusher writes all 8 cached bytes, including the 5 that were never fetched.

Other spans are fine: a span that starts inside a block fetches it, and so does a span that ends inside a different block. `eeprom_read_bytes` fetches every block it touches. EEPFS isn't affected in practice. With checksum or backup, `eepfs_write` writes the data first. The data starts after the header, inside the block, so the block is fetched before the short header write. Without them, a file shorter than a block hits the bug, but the bytes it loses are that file's own padding.

Note for `trunk` (e356bf3f5, eeprom.c unchanged since 2c3d5db): the older implementation has a different bug with the same kind of span. Lines 94 and 125 subtract `8 - block_offset` from `bytes_left` even when `len` is smaller. So `eeprom_read_bytes(dest, 21, 1)` copies 3 bytes into `dest`, and then `bytes_left` wraps around and the whole-block loop practically never ends. `eeprom_write_bytes` has the same arithmetic. `preview` replaced that code in #843, so this only matters if `trunk` gets fixes on its own.
