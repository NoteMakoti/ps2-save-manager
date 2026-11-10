# PS2SM network protocol (v1)

The PS2 homebrew app (`ps2/`) runs a TCP server; the desktop app and the
CLI connect to it. The protocol is a simple framed request/response
protocol, one request in flight at a time per connection.

* Default port: **6524/tcp**
* All integers are **little-endian**
* `str` = `u16 length` + bytes (no terminator). Paths and names are ASCII.
* `tod` = the 8-byte PS2 timestamp used in directory entries
  (`unused, sec, min, hour, day, month, year:u16`, JST).
* Maximum payload: 16 MiB.

## Framing

Request header (16 bytes):

| off | size | field                         |
|-----|------|-------------------------------|
| 0   | 4    | magic `"PS2S"`                |
| 4   | 1    | protocol version (`1`)        |
| 5   | 1    | command                       |
| 6   | 2    | reserved (0)                  |
| 8   | 4    | request id (echoed back)      |
| 12  | 4    | payload length                |

Response header (16 bytes):

| off | size | field                                     |
|-----|------|-------------------------------------------|
| 0   | 4    | magic `"PS2R"`                            |
| 4   | 1    | protocol version                          |
| 5   | 1    | command (same as request)                 |
| 6   | 2    | status, `i16`: 0 = OK, < 0 = error        |
| 8   | 4    | request id                                |
| 12  | 4    | payload length                            |

On error the payload is a `str` with a human readable message. Status
codes follow the libmc / errno convention where possible:

| status | meaning               |
|--------|-----------------------|
| -1     | generic failure       |
| -2     | not found             |
| -3     | no card / unformatted |
| -4     | no space              |
| -5     | already exists        |
| -6     | bad request           |
| -7     | not supported         |
| -8     | I/O error             |

`port` below is the memory card port: 0 = slot 1, 1 = slot 2.

## Commands

| id   | name          | request payload                                   | response payload |
|------|---------------|---------------------------------------------------|------------------|
| 0x01 | HELLO         | `str client`                                      | `u16 proto, str server_name, str version, u32 caps` |
| 0x02 | CARD_INFO     | `u8 port`                                         | `u8 present, u8 type, u8 formatted, u8 pad, u32 free_kb, u32 total_kb` |
| 0x03 | LIST_DIR      | `u8 port, str path`                               | `u32 n`, n × `(str name, u16 mode, u32 size, tod created, tod modified)` |
| 0x04 | READ_FILE     | `u8 port, str path`                               | raw file bytes |
| 0x05 | WRITE_FILE    | `u8 port, str path, u32 len, bytes`               | — |
| 0x06 | MKDIR         | `u8 port, str path`                               | — |
| 0x07 | DELETE        | `u8 port, str path` (file or empty dir)           | — |
| 0x08 | GET_SAVE      | `u8 port, str dirname`                            | the save as a `.psu` image |
| 0x09 | PUT_SAVE      | `u8 port, u8 overwrite, psu bytes…` (rest)        | — |
| 0x0A | DELETE_SAVE   | `u8 port, str dirname` (recursive)                | — |
| 0x0B | FORMAT        | `u8 port`                                         | — |
| 0x0C | LIST_SAVES    | `u8 port`                                         | `u32 n`, n × `(str dir, str title_sjis, u32 size, tod modified)` |
| 0x0D | MMCE_STATUS   | `u8 port`                                         | `i32 card, i32 channel, str game_id` |
| 0x0E | MMCE_SET      | `u8 port, u8 what (0 card, 1 channel), u8 mode (0 set, 1 next, 2 prev), u8 pad, u16 number` | `i32 result` |
| 0x0F | BYE           | —                                                 | — (server closes) |

Capability bits returned by HELLO:

| bit | meaning                                   |
|-----|-------------------------------------------|
| 0   | memory card access (always set)           |
| 1   | MMCE device present (MemCard PRO2, SD2PSX) |
| 2   | USB mass storage available                |
| 3   | SMB share mounted                         |

`card type` follows libmc: 0 none, 1 PS1, 2 PS2, 3 PocketStation.

### Notes

* `PUT_SAVE` with `overwrite = 0` fails with `-5` if the directory exists.
  The server writes the files, then applies the original timestamps and
  attributes with `mcSetFileInfo`.
* `GET_SAVE`/`PUT_SAVE` use the `.psu` layout as the transfer format (512
  byte directory entries + data padded to 1 KiB), so a save can be written
  to disk unchanged.
* After `MMCE_SET` the console needs ~1 s before the new card answers;
  clients should wait before issuing `CARD_INFO`.
