# Protocol
The WebSocket between the client (`src/client/src/net.ts`) and the server (`src/server/src/protocol.rs`), which change together. Each player holds one connection, at `ws` relative to the page so a reverse proxy can mount the game at any path. Text frames are JSON objects tagged by `type`. The two per-tick streams are binary frames, all little-endian.

Clients are untrusted, so the server bounds what one can cost it. A client message over 256 bytes closes its connection: the largest legitimate one, a join with a 24-character name, is about 120. A connection with 256 frames waiting to go out, about 8 s of pose frames, is disconnected as too slow, as is one whose frame takes 10 s to send.

| From | Message | Carries |
|---|---|---|
| Client | `join` | The typed `name`, as the connection's first message. A name that is empty, over 24 characters, or holds a control character gets `error` and a closed connection |
| Client | `start` | The host's `cake`, an id or `random`. Refused with `error` from anyone else, outside the lobby, or for a cake the server lacks |
| Client | `ready` | Sent once the round's cake has loaded |
| Client | input, binary | Per tick, the `u32` tick the input steps the world from, then `Input::encode`'s nine `i32` |
| Client | `cut` | The `tick` the world was at when `Cut!` was pressed, after that tick's inputs |
| Client | `hash` | `World::calc_hash` at `tick`, once a second, as a decimal string since a JSON number cannot hold every `u64` |
| Server | `welcome` | The connection's own `id`, once |
| Server | `room` | `phase`, `host`, and `members`, whenever one changes |
| Server | `round` | The round's `cake`, `round_ticks`, and `players`. Anyone not listed spectates |
| Server | `start` | Tick 0, `elapsed_ms` ago for a spectator who joined late |
| Server | `cut` | A player's cut `tick` and frozen `pose`, and the `cut_lower_m`, `cake_touch_lower_m`, and `is_cut_through` the cut animation needs, so every client can replay it. Never the score |
| Server | pose, binary | During play, about 30 times a second, a 56-byte record per round player: `u32` id, tick, and flags (1 holding, 2 cut), then `f32` cake translation and rotation quaternion, knife tilt, lowering, and rest, and stand angle |
| Server | `results` | Every round player's `standings`, once all have cut or the deadline passed |
| Server | `desync` | The client's hash at `tick` differs from the server's replay |
| Server | `error` | Why a message was refused |

## Round
The room moves from `lobby` to `loading` when the host starts, to `playing` once every player is `ready` or 10 s have passed, to `results` once every player has cut, and back to `lobby` 10 s later. A player who joins in any phase but the lobby gets the round, its start, the cuts so far, and the results if they are showing, then spectates and plays the next round. A player who leaves mid-round drops out of the round and its results, and if the host leaves, the longest-present player becomes host.

The server replays each player's messages in the order they arrived, at its own 60 Hz step. A message stamped ahead of the replay is reached by repeating the player's last input, and a stale input is dropped, both logged. An input or cut stamped more than 30 ticks (0.5 s) past the round's clock, the ticks of wall time since `start`, is refused and logged, so no message makes one step replay a world far ahead. The deadline is the round's length plus 2 s of grace after `start`: then the server finishes every world that has not cut by holding its last input to the round's last tick, where the world cuts itself, and drops every later input.

The client keeps to the server's clock, counted from when `start` arrived less its `elapsed_ms`. Each frame it steps the newest few ticks with the live controls, and any older ones it missed, while its tab was hidden, its frames ran slow, or its cake loaded after the start, with the input it sent last, which is what the server holds for a tick it has no input for. It sends every input it steps, so the server's world matches what the player saw whether those inputs arrive before the deadline or the deadline holds the same input. The catch-up spends at most 40 ms a frame, and `Cut!` stays disabled until it ends. Results that arrive before the player's own world has cut are shown once it does, so a player back after the deadline sees the timeout cut the server scored first.

Standings rank a higher unrounded score first, and of equal scores the earlier cut. Cuts equal in both share a rank. Players who never touched the controls share a rank because their worlds run alike to the same timeout cut, and `has_touched` marks them.
