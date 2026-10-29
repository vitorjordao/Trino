eeprom: read the block before a write that ends inside it

Filed as https://github.com/n64brew/libdragon/pull/9

Fixes #8.

`eeprom_write_bytes` reads a block back before a partial write only when the write starts inside it, or ends inside a *different* last block. A write that starts on a block boundary and ends inside the same block skips the read-back. Example: `eeprom_write_bytes(src, 136, 3)`, i.e. block 17, bytes 0..2. If that block isn't cached yet, bytes 3..7 come from uninitialized cache memory, and the flush writes them to the EEPROM.

```
block 17   [ 10 | 11 | 12 | ?? | ?? | ?? | ?? | ?? ]   <- cache
             \__________/   \____________________/
             written (len=3) never fetched, flushed
```

The fix fetches the last block whenever the write ends inside it (`end_offset != 0`). When the last block is also the first one, `eeprom_cache_ensure_block_valid` returns early on the second call because the block is already valid.

## Tests

Two new testrom tests, `test_eeprom_read_bytes` and `test_eeprom_write_bytes`. Each one goes through seven spans, written as (offset in the block, length):
- inside one block: `(5,1)`, `(1,3)`, `(0,3)`
- to the end of a block: `(3,5)`
- across a boundary: `(6,4)`, `(0,11)`
- one whole block: `(0,8)`

For each span, the tests:
1. seed blocks the cache hasn't touched yet with known bytes, using raw Joybus commands;
2. call `eeprom_read_bytes` / `eeprom_write_bytes`, then `eeprom_wait_idle`;
3. check the hardware contents, again over raw Joybus.

They check that only the span changed, including the blocks on either side, and that a read writes nothing outside its destination. The tests skip without EEPROM, like `test_eepromfs`, so the ROM has to be built with a save type: `make -C tests N64_ROM_SAVETYPE=eeprom4k`.

ares v148, `testrom_emu.z64`, `N64_ROM_SAVETYPE=eeprom4k`.

Before the fix (f26e1c161 with the new tests):

```
**** Starting test: test_eeprom_read_bytes
**** Starting test: test_eeprom_write_bytes
write_bytes(start=77, len=1)
write_bytes(start=105, len=3)
write_bytes(start=136, len=3)

ASSERTION FAILED (test_eeprom.c:134):
[1112000000] != [11128b8c8d]
     ^^              ^^  idx: 11
write_bytes(136, 3) changed the wrong bytes

**** Starting test: test_eepromfs
...
Passed: 220 out of 222 (7 skipped)
```

After the fix (same result with `eeprom16k`):

```
**** Starting test: test_eeprom_read_bytes
**** Starting test: test_eeprom_write_bytes
**** Starting test: test_eepromfs
...
Passed: 221 out of 222 (7 skipped)
```

The remaining failure is `test_timer_ticks`. It fails the same way before and after this change, and it is unrelated.
