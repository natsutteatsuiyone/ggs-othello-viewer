# GGS Othello observing protocol

What the viewer sends to a GGS server and its Othello service `/os`, and how it reads the replies.
Sections 1 to 6 describe the formats and what the viewer does with them. Section 7 records what a live
server sent and what the GGS server source says about it. File references such as `Game.C:1160` point
into the GGS server source (section 7).

---

## 1. Transport, login, post-login commands

### 1.1 Server and service

The login form starts with `www.skatgame.net`, port 5000. The Othello service is the login **`/os`**;
every service command is a tell to it.

### 1.2 Line framing

* Plain TCP socket.
* **Read:** bytes are accumulated until the two-byte sequence CR LF; the CRLF is dropped and the bytes
  are decoded as UTF-8. A CR not followed by LF is dropped and the following byte kept. A line over
  64 KiB or a message over 4 MiB ends the connection, and so does EOF.
* **Write:** the UTF-8 bytes of the line, then a single LF. No CR is sent.
* The read timeout is a third of the keepalive interval and only lets the viewer check for a silent
  server (section 7, **Keepalive**).

### 1.3 Login handshake (before the message loop)

Lines are read one at a time:

1. A line that **ends with** `use).` is the login prompt (`: Enter login (yours, or one you'd like to
   use).`): send the login name. A second prompt means the name was refused; the login fails with
   "illegal login name".
2. After the name, a line that **contains** `password`: send the password, which is empty for a new
   name (section 7, **Login**). A second password prompt fails the login with "password rejected".
3. After the name, a line that is exactly `READY`: the handshake is done.
4. Any other line is ignored.

### 1.4 Commands sent after login

To the server directly, in order:

```
verbose +ack
vt100 -
notify + *
notify +
```

`verbose +ack` makes the server echo every command as an ack (section 7, **Acks**).

When the `/os` service is announced as present (see 2.4), the viewer sends:

```
tell /os vt100 -
tell /os client +
tell /os notify + *
tell /os notify +
tell /os watch +
tell /os match
```

`tell <user> <text>` is the GGS tell command. The viewer writes `tell` in full because new accounts
have no `t` alias (section 7, **Login**). It does not send `open 1`, which would accept match requests.
What `client +` and `watch +` without an id do is not documented.

---

## 2. Splitting server output into messages and routing

### 2.1 Message block grammar

```
stream    := { block }
block     := { "" }  line { line }  terminator      ; empty lines before a block are skipped; empty lines inside a block are kept
terminator:= "READY" | "ALERT"                        ; whole-line exact match
```

Processing of a completed block (list `L` of lines including the terminator):

1. If `L` has only the terminator, it is discarded.
2. If `L[0]` does **not** start with `:`, then for every `L[i]`, `i >= 1`, a single leading `|` is
   removed (continuation prefix). `L[0]` is never stripped, and blocks whose first line starts with `:`
   keep their `|`.
3. The terminator line is removed.
4. All BEL characters (0x07) are removed from every line.
5. The remaining lines form one message.

So a message is `Vec<String>`: header line plus zero or more continuation lines.

### 2.2 Routing

The header line decides the kind of message:

| Header | Message |
|---|---|
| `: + <login> ...` / `: - <login> ...` | presence notice (2.4) |
| `: help <topic>`, `/os: help <topic>` | help file |
| `: finger`, `: finger ERR ...` | the server's own profile reply |
| `/os:` | the Othello service (section 4) |
| `/td:` | the tournament director (section 7, **Tournaments**) |
| `.<channel> /td: ...` | a tournament broadcast on a channel |
| `: ERR user '/os' not found.` | `/os` is not logged in |
| `.<channel> <login>: <text>`, `<login>: <text>` | channel line or tell (section 7, **Channels**, **Tells**) |
| `.<channel> <login>: <text>` on `<login>`'s own log channel | an engine log line, kept out of the chat (section 7, **Engine logs**) |

Anything else is shown only in the log pane.

### 2.3 Header-line tokenization for `/os:` messages

`L[0]` is split on whitespace (runs of spaces collapse):

```
header := "/os:" SP keyword { SP token }
```

Token 0 is `/os:`, token 1 is the keyword that selects the message type (section 4).

### 2.4 Presence notices

A line that starts with `:`, has `+` or `-` at index 2 and a space at index 3 is a presence notice:

```
central_line := ":" SP ("+"|"-") SP user [ SP extra ]     ; lines with more than 4 tokens are skipped
```

`: + /os ...` means the service is present, and the viewer then sends the `/os` commands of 1.4;
`: - /os ...` means it left. The viewer sends no `/os` command before `/os` is present. An ack line
(`: +ack: ...`) is not a presence notice (section 7, **Acks**).

---

## 3. Commands the viewer sends

| Purpose | Exact line |
|---|---|
| List running matches | `tell /os match` |
| Observe a match | `tell /os watch + <id>` |
| Stop observing | `tell /os watch - <id>` |
| Who is online per type | `tell /os who 8 a`, `tell /os who 8r a` |
| Top ratings | `tell /os top 8 24`, `tell /os top 8r 24` |
| A user's page | `finger <login>`, `tell /os finger <login>`, `tell /os history <login>`, `tell /os stored <login>` |
| Replay an archived game | `tell /os look <key>` |
| Help | `help`, `help news`, `tell /os help` |
| Kibitz to a game | `tell /os tell <id> <text>`, or `tell /os tell o <id> <text>` for observers only |
| Tournaments | `tell /td tourneys`, `tell /td finger <id>`, `tell /td st <id>`, `tell /td rankings <id>` |
| Follow or stop following engine logs | `chann + .<a> .<b>`, `chann - .<a> .<b>` (section 7, **Engine logs**) |

`<id>` is the first token of a match-list entry. Game ids start with `.` (e.g. `.12`).

**Getting the board on join:** no extra command. After `watch + <id>` the server sends a `join` message
carrying the full GGF game so far (section 4.2), and the viewer builds the game from it.

---

## 4. Server messages from `/os`

Notation: `L[0]` header, `L[1..]` continuation lines (already `|`-stripped). `tok[k]` = k-th token of
`L[0]`.

### 4.1 `match` (full match list)

```
L[0] = "/os: match" [ ... ]          ; rest of L[0] ignored
L[i] = <match-entry>                  ; one per running match
```

The list replaces the previous one. Section 7, **Match list**, has the layout of an entry.

### 4.2 `join` (initial game state; also used for re-join)

```
L[0]  = "/os: join" SP <id> { SP <ggf-fragment> }
L[1..]= <ggf-fragment>
```

The GGF text is `tok[3..]` of `L[0]` plus every `L[i]`, joined with spaces, parsed as in section 4.9.
A game already held under that id is replaced.

Example (reconstructed):
```
/os: join .12
|(;GM[Othello]PC[GGS/os]DT[2006.08.17_12:00:00.GMT]PB[alice]PW[bob]RB[1850.00]RW[1720.50]
|TI[05:00//02:00]TY[8]RE[?]BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]
|B[d3//3.10]W[c5/-1.23/4.5];)
READY
```

### 4.3 `update` (one live move)

```
L[0] = "/os: update" SP <id> SP <move-token> { SP ignored }
```

`tok[2]` selects the game by exact id; `tok[3]` is the move (section 4.10). A missing id or move is an
error. Example: `/os: update .12 f4/0.50/2.73` (reconstructed).

### 4.4 `+ match` / `- match` (match started / finished)

```
"/os: + match" SP <match-entry>
"/os: - match" SP <id> SP <rest>  [L[1..] text]
```

`+ match` adds an entry to the running list. `- match` removes the entry for `<id>`; `<rest>` repeats
the entry and adds the result (section 7, **Game over**, **Archive counter**). The ended match stays in
the list's finished part, which keeps the last 20. A match ending also ends the games `<id>.0` and
`<id>.1` of a synchro match.

### 4.5 `+ <name> watch <id>` / `- <name> watch <id>` (observer joined / left)

```
"/os: +" SP <name> SP "watch" SP <id>
"/os: -" SP <name> SP "watch" SP <id>
```

They apply to `<id>`, `<id>.0` and `<id>.1`. Each game keeps a deduplicated observer set.

### 4.6 `end` (game over)

```
L[0]  = "/os: end" SP <id> ...
L[1..]= result text
```

A live server sends `end` only for each board of a synchro match; a normal game ends with `- match`
alone (section 7, **Game over**). The GGF `RE[]` value carries the result of a replayed game.

### 4.7 `rating_update <id>`

`tok[2]` = id; `L[1..]` is rating text kept with that game.

### 4.8 Other per-game text messages

| Header | Content |
|---|---|
| `/os: <.id> <text>` (keyword starts with `.`) | kibitz (section 7, **Kibitz**) |
| `/os: undo\|score\|abort <id> <login> ...` | a player's request (section 7, **Game notices**) |
| `/os: timeout\|fatal-timeout\|trust-violation <id> <login> (<color>) ...` | a clock notice (section 7, **Game notices**) |
| `/os: comment\|error <id> <text>` | text kept with that game |
| `/os: look` + `L[1] = <n> <ggf>...` | replay of an archived game: `n` GGF records back to back, split after the first `;)` when `n` is 2 (section 7, **look**) |
| `/os: + .<id> <rating> <player> <clock> <type> ...` | an incoming match request, sent to players only |

### 4.9 GGF game record (inside `join` / `look`)

Lexing:

* Skip to the first `(` immediately followed by `;`.
* Separators between properties: whitespace and `;`. A `)` ends the record.
* Property name: the characters up to `[`, trimmed and upper-cased.
* Value: everything up to the next `]`; no escaping.

Properties:

| Prop | Meaning |
|---|---|
| `PB` / `PW` | black / white player name |
| `RB` / `RW` | black / white rating, float |
| `DT` | date string, kept verbatim |
| `TY` | game type string (section 6.2) |
| `TI` | clock setting for both colors (section 5.1) |
| `TB` / `TW` | separate black / white clock settings |
| `KM` | komi value, float; marks komi as decided |
| `KB` / `KW` | black / white komi bid, `move/komi/time` |
| `RE` | black's disc margin, `?` while unfinished, optionally `:r`, `:s` or `:t` (section 7, **look**) |
| `BO` | start position (section 4.11); only one allowed |
| `B` / `W` | a move (section 4.10); requires `BO` earlier |
| anything else | skipped, so new server fields do not break viewing |

`B` and `W` are handled identically: the side that moves is always the board's side to move. A record
without `BO` is an error.

If `KM` is present, the time field of the komi bid of the side that does not start is charged to that
side's clock before the first move; a missing bid counts as zero time (section 6.4).

### 4.10 Move token

```
move-token := move [ "/" [eval] [ "/" [time] ] ]
move       := square | "PA" | "PASS" | "%"             ; case-insensitive
square     := column-letter row-number                 ; A = leftmost column, row 1 = top row, e.g. d3, F5, a10
eval       := float                                     ; engine evaluation in discs
time       := float                                     ; seconds used for this move, >= 0
```

Empty fields are allowed (`d3//3.10`, `d3/`) and read as 0; an empty eval is also marked as not
stated, since a player that writes no evaluation is not claiming 0. A missing move, a non-numeric or
negative time, or a third `/` is an error. Passes are explicit moves.

### 4.11 Board text (`BO[...]`)

```
BO-value   := board-type { ws } squares { ws } color
board-type := int          ; must start at the first character
squares    := WIDTH*HEIGHT square-chars in row-major order, row 1 first; whitespace between them ignored
square-char:= "*" black | "O" white | "-" empty       ; any other char is an error
color      := "*" | "X" | "V"  → black to move
            | "O" | "H"        → white to move        ; case-insensitive
```

Board types 4 to 14 give a square board of that size; `88` is the 10x10 octagon, whose 12 corner squares
(0,0)(0,1)(1,0)(9,9)(9,8)(8,9)(0,9)(0,8)(1,9)(9,0)(8,0)(9,1) are off-board and **absent from the
string**. Text after the color is an error.

Standard 8x8 start example: `BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]`.

---

## 5. Clocks

### 5.1 Clock setting string

```
clock   := part1 [ "/" part2 [ "/" part3 ] ]
part    := [ hms ] { "," ignored }             ; only text before the first "," is used
hms     := sec | min ":" sec | hour ":" min ":" sec     ; integers
part1   = initial time,  default 15:00 (900 s)
part2   = increment per move, default 0
part3   = extension (one-time bonus), default 2:00 (120 s)
```

**An empty field keeps the default, it does not mean zero.** So `05:00//02:00` = 300 s initial,
0 increment, 120 s extension; `05:00//` = 300 s, 0, **120 s**. Totals above 86400 s, fractional
seconds and a fourth field are rejected. The viewer writes a setting with every field filled in, e.g.
`5:00/0:00/2:00`, since an empty field would read back as the default.

### 5.2 Clock arithmetic

When a move with time `t` seconds is recorded for the side to move, `t` is subtracted from its clock;
if the result is negative and the extension is unused, the extension is added and used up; then the
increment is added. This is the server's order (section 7, **Clock**).

The remaining time is **reconstructed** from the clock setting plus the `time` field of every move (GGF
moves on join, `update` moves live). The server does not send remaining-time values in `join` or
`update`.

### 5.3 Live ticking

* The display is redrawn every 0.98 s, so the displayed second never skips.
* Only the live position of a live game has running clocks: the side to move, or both sides while a
  komi game's komi is not decided.
* A running clock shows its value after the last move, truncated to whole seconds, minus the time since
  that move. Once it passes zero it shows the extension added, as the server's timeout does.
* Format: sign `+`/`-`, then `H:MM:SS`. Example: `+0:04:57`, `-0:00:03`.
* On join the time already spent on the current move is unknown, so the running clock reads high and is
  marked `~` until the next move.
* A synchro board's clock does not tick: it shows the last confirmed balance, and the time since the
  last move is shown separately.

---

## 6. Othello-specific rules and type strings

### 6.1 Board sizes

Board types 4, 6, 8, 10, 12, 14 and 88 (octagon); 8 is the usual one (section 4.11).

### 6.2 Game type grammar (`TY[...]`)

```
type     := [ "s" ] board-int { modifier }            ; case-insensitive
modifier := "k"                ; komi game (no color preference allowed)
          | "r" int            ; random start
          | "a"                ; anti (reverse) Othello
          | "b" | "w"          ; preferred color black / white, at most one, not combined with k or r
```

Examples: `8`, `10`, `88`, `s8`, `8k`, `8r20`, `s8r18`, `8a`, `8b`. The start position is always given
by `BO[]`, so the viewer reads only `s` (synchro) and `k` (komi) from the type.

### 6.3 Synchro games

A synchro match `.N` is played as two simultaneous games with ids `.N.0` and `.N.1`. `update` and
`join` use the exact sub-id, while `+/- watch`, `- match` and `.N` text lines apply to both boards.
`look` returns both GGF records back to back, split after the first `;)`.

### 6.4 Komi games

Before the komi is decided, both clocks run. The decided komi is `KM[]`. The GGS server stores the
starting side's bid as the first move once komi is decided (`Game.C:1160-1172`) while still writing
both `KB`/`KW` (`Game.C:619-623`), so only the other side's bid is charged at parse time; charging both
would count the starter's bid time and increment twice.

### 6.5 Anti games

`a` in the type marks anti Othello. Move legality and flipping are identical to normal Othello; only
the scoring goal differs.

---

## 7. Verified against a live server

Observed on 2026-09-30 and 2026-10-01 against a local GGS (the docker image with `/os` and the `edax`
bot), port 5000. Raw traffic from seven sessions is kept in `fixtures/live-normal.txt`,
`fixtures/live-synchro.txt`, `fixtures/live-notify.txt`, `fixtures/live-kibitz.txt`,
`fixtures/live-archive.txt`, `fixtures/live-chat.txt` and `fixtures/live-tourney.txt`; the tests in
`src/net.rs`, `src/proto.rs`, `src/lobby/tests.rs`, `src/td.rs` and `src/app/` replay them.

File references such as `Match.C:285` point into the GGS source the docker image is built from
(`Service/GameLib/src/`). Formats taken only from the source, not yet seen live, are marked *source only*.

* **Login.** A new name gets `: Enter your password.` too and must answer with an empty line. There is no
  `t` alias for new accounts: `t /os match` returns `: ERR tell <who1,...>`, so every service command is
  sent as `tell /os ...`.
* **Acks.** Every command is echoed as `: +ack:  <cmd> => <cmd>` in its own READY block. It is not a
  presence notice (its third byte is `a`, not a space).
* **Presence.** The core `notify + *` is answered with one block of `: + <login> <0|1>` lines, e.g.
  `: + /os 1`, `: + edax 1`, `: + pview1 0`. `: + /os 1` is what triggers the `/os` setup commands.
* **Keepalive.** A bare `:` is answered with an empty READY block. Once logged in, the viewer sends `:`
  after 45 s without sending any other line; since every keepalive gets a reply, 135 s with nothing
  received ends the connection.
* **Match list.** Header `/os: match <n>/<n>`, entries `  .4 1720 pbot     1720 edax            8  U 0`,
  that is `id rating1 player1 rating2 player2 type mode count`, space-padded. `match 0/0` has no entries.
  The mode is `U`, `R` or `T` (tournament), and the count is the number of observers
  (`VC_Match.C:377-383`). The viewer does not show the count: it goes stale, because watch notices only
  arrive for watched games.
  A synchro match is listed once as `.5 ... s8r10  U 1`; `.5.0` and `.5.1` never appear in the list.
* **Match start.** `/os: + match .6 1720 pbot 1720 edax 8 U` (no count) arrives with only
  `tell /os notify + *` set (`Match.C:182`).
* **Join.** `/os: join .4 (;GM[Othello]PC[GGS/os]DT[...]` with the rest of the GGF on `|` lines, one tag
  group per line, ending `|;)`. Move tags may be upper case (`B[E6]`). `watch + .5` on a synchro match
  yields `join .5.0` and `join .5.1`, and the colors are swapped in `.5.1`. `TI`/`TB`/`TW` carry the
  clock *setting*, not the time left (`Game.C:604-613`), so the time already used by the side to move
  when the join arrives is unknown.
* **Join again.** *Source only.* A game already being watched is sent a fresh `join` when an undo is
  accepted (`Game.C:836`) and when both komi bids are in (`Game.C:1185`). The viewer keeps the observers
  and kibitz across it. `DT` is the send time (`Game.C:592`), so changing it does not identify a new match.
  Komi assignment can swap `PB` and `PW`. A new `+ match` retires any held predecessor with the same ID;
  the server sends it before the initial joins (`EXE_Service_callback.C:82-95`).
* **Joining a finished synchro board.** *Source only.* `join` can already contain a numeric `RE` for one
  finished board while its partner is still running (`Game.C:632-640`, `Match.C:637-678`). Its clocks
  stop and its result stays separate from the final match score. If the boards are out of sync, the
  server can omit its last move while retaining `RE`; the later `update` must still be applied
  (`Game.C:717-737`).
* **Stop and watch again.** `tell /os watch - .15` is acked and nothing more arrives for `.15`.
  `watch + .15` later gets a new `join` with every move from move 1.
* **Update.** Tokens seen: `d3//6.00` (empty eval), `f3/11.00/0.10`, `b5/17.00` (no time), `pa//1.15`
  (lower-case pass).
* **Game over.** A normal game gets no `end`, only `/os: - match .4 1720 pbot 1720 edax 8 U -64.00`. The
  score is from player 1's side as listed in the entry, not black's: in `.6` edax was black, pbot listed
  first resigned, and the score was `-64.00`. A synchro game gets
  `/os: end .5.0 ( pbot vs. edax ) -64.00` for each board and then `- match .5 ... -64.00`.
  *Source only:* a rated match adds a global counter after the score, `... R +30.00 17`
  (`Match.C:294`, `Match.C:316`). An unfinished match ends `... aborted` or `... <login> left`
  (`Match.C:306`, `Match.C:310`); the synchro `end` lines for those are `( a vs. b ) aborted` and
  `( a vs. b ) player <login> left` (`Match.C:616-618`, `Match.C:575-577`).
* **Watch notices.** `/os:  + pobs1 watch .15`, with two spaces after the colon (the prefix is `" + "`,
  `EXE_Service_watch.C:47`, `EXE_Service_watch.C:156-157`). A new watcher is told about everyone
  already watching; the notice for one's own watch is not sent. Those notices precede `join`
  (`EXE_Service_watch.C:166-169`) and apply to both boards of a synchro match.
* **Kibitz.** `/os: .14 A 1720 pkib2: hello all`: the id as the sender gave it, the channel, the
  sender's rating padded to four places, then `<login>: <text>` (`EXE_Service.C:1203`). The sender does
  not have to watch the game. `tell /os tell .14 x` and `tell /os tell a .14 x` both arrive as `A`,
  `tell o` as `O`, and `tell p` goes to the two players only, so a watcher never sees it
  (`EXE_Service.C:1210`).
* **Game notices.** *Source only.* `undo .4 pbot is asking` (`Game.C:819`), `score .4 pbot is offering`
  (`Game.C:899`), `abort .4 pbot is asking` (`Match.C:631`), `timeout .4 pbot (*)` (`Game.C:1283`),
  `fatal-timeout .4 pbot (O)` (`Game.C:1315`), `trust-violation .4 pbot (*) delta= 10 + 3.2 secs`
  (`Game.C:1093`) and `comment .4 <text>` (`Game.C:1232`). The color is `*`, `O`, or `?` before komi is
  decided. The viewer turns each into a sentence in the kibitz pane.
* **Clock.** *Source only.* After a move the server subtracts the time used, takes the extension if the
  result is negative, and then adds the increment (`GAME_Clock.C:160-191`). The extension is used once.
* **who.** `/os: who     4      change: ...` then rows like
  `edax     - 1720.0@350.0 ->  +324.4    +0.0  -324.4 @290.2 .4`; a trailing `.N` is the match that user
  is in. The `/os` row is listed too.
* **finger.** One `/os: finger <name>` block per user even when several are asked for, with lines like
  `login  : edax` and `rated  : +`. The replies can arrive before the `who` that was sent first.
  `open` is the simultaneous-match limit and `play` lists active matches. `accept` and `decline` are
  the user's automatic match-request formulas; an empty formula evaluates to false. `ignore` lists
  blocked players. The viewer keeps these settings and sends `tell /os finger <login>` (with `finger`,
  `tell /os history` and `tell /os stored`) when opening a user page. The server checks ignore lists, rated compatibility and free slots before evaluating
  auto-decline, then auto-accept (`VAR_Client.C:87-111`, `VC_Formula.C:88-97`, `VC_Request.C:446-481`).
* **Server finger.** The server's own `finger edax` (not `tell /os finger`) answers `: finger` and then
  `key : value` lines with no `|`: `login`, `dblen`, `name`, `info`, `since` and
  `idle   : 00:00:22, on line : 07:50:10` while the login is on (`last` and `after` when it is off), then
  `bell`, `verbose`, `vt100`, `hear`, `groups(-)`, `channs(-)`, `notify(-)` and `ignore(-)`; `passw`,
  `email`, `host` and `sock` appear only for oneself or a higher admin (Central `VAR_Client.C:222-273`).
  A multi-line info repeats `info   :` (`EXE_Client.C:374-383`). An unknown login gives
  `: finger ERR not found: nobodyxx`. The ack ends the same block. Captured on a local server.
* **Errors.** `/os: ERR match '.4' not found.` (telling a finished game) and `/os: watch + ERR not found: *`.
* **Rated games need a registered login.** `tell /os rated +` from a new name answers
  `/os: ERR you are not registered.` (`EXE_Service.C:1519-1521`, `Message.C:220-222`). Only rated
  matches are archived (`Match.C:238-275`), so `history`, `stored`, `look` and `top` stay empty until a registered, rated
  login has played. The captures used the login `prated`, registered by root with `name` and `email`
  after logging in once with a password.
* **Archive counter.** A rated match ends `/os: - match .23 1402 prated 2038 edax 8 R -64.00  .73`: two
  spaces, then the key the match was archived under (`Match.C:315-316`). It is a separate counter, not
  the match id (`VC_Match.C:66-79`). An unrated match has no counter and cannot be looked up again.
  *Source only:* a rated match adjourned because a player left ends `... <login> left .69` and is kept as
  a stored game; the capture has the stored row and its `look`, not the `- match` line.
* **history.** `tell /os history [login ...]` answers one block per login,
  `/os: history 4 prated` and then rows like
  `.73      01 Oct 2026 03:42:15 1402 prated   2038 edax      -64.0 8`: key, date, rating and login of
  player 1, of player 2, player 1's margin and the type (`GAME_History.C:39-50`,
  `EXE_Service.C:1307-1336`). An unknown login gives `/os: history ERR not found: nosuchxx`.
  Rows run oldest first, and only a login's last 32 games are kept (`VAR_Client.H:43`,
  `GAME_History.C:109`), so the games of a long tournament soon fall out. A tournament's page therefore does not look its
  games up in history; a user's page still replays any game in that user's history.
* **stored.** `/os: stored 1 prated` and rows like `.69      01 Oct 2026 03:35:41 prated   edax     8:l`:
  key, date, the two logins and the type, with `:l` when a player left (`GAME_Stored.C:39-50`,
  `Match.C:271`).
* **look.** `tell /os look .73` answers `/os: look` and one `|` line holding `<n>` and `<n>` GGF records
  on one line (`EXE_Service.C:1285-1303`, `Match.C:241-243`). `n` is 1, or 2 for a synchro match, whose
  records follow each other directly (`...;)(;GM[Othello]...`) and are split after the first `;)`. The reply does not repeat the key. `RE` is black's
  margin `%+.3f`, with `:r`, `:s` or `:t` for a resignation, an agreed score or a timeout, or `?` for an
  unfinished game (`Game.C:627-634`); the captured ones are `RE[-64.000]`, `RE[+64.000]` and `RE[?]`. A
  missing key gives `/os: ERR no archived match '.17' found.`, `look` with no key
  `/os: ERR no archived match '' found.`
* **top.** `tell /os top 8 24` answers a header with the average rating and the number ranked, then one
  row per player: `    1 edax     2072.7@278.0=     00:00:11+@278.0 +64.0      2      0      0`, that
  is place, login, rating `@` deviation, time since the last game, `+@` deviation, average result, wins,
  draws and losses (`VC_Rank.C:199-214`, `VC_Rank.C:288-307`, `VC_Rating.C:141-151`). The fields are
  printf-padded: a deviation under 100 leaves a space after `@`, and an average of six characters runs
  into the deviation before it. The reply does not name the type. `top 8r 24` lists the `8r` pool,
  which synchro `s8r20` games are rated in. With nobody ranked the header ends ` 0` and no rows follow.
* **who 8r.** `tell /os who 8r a` has the same layout as `who 8` and does not name the type either; the
  ratings are those of the `8r` pool.
* **Help.** The server sends `: help faq`, `: help help` and `: help news` unasked right after login.
  `help` and `help news` answer `: help help` and `: help news` blocks whose last line is the ack of the
  command. `tell /os help` answers `/os: help help` with `|` continuation lines.
* **Core replies.** `who` answers `: who    4        idle      online         ip-addr hostname` and one
  row per login; an unknown command gives `: ERR command 'frobnicate' not recognized.` The ack of a
  command is in the same block when the reply comes from the server itself, and in its own block when
  it comes from `/os`.
* **Channels.** Every login starts in `.chat` and `.help` (`finger` shows `channs(-) : .chat .help`), so
  nothing has to be joined. `tell .chat reply from b` sent by pchat2 reaches the others as
  `.chat pchat2: reply from b`: channel, sender, colon, text (Central `VC_SEQ.C:52-77`). The sender does
  not get its own line back (Central `VC_SEQ.C:64`).
* **Tells.** `tell pchat1 hi a` from pchat2 reaches pchat1 as `pchat2: hi a`, that is `<sender>: <text>`
  (`EXE_Client.C:561-562`). A tell to oneself comes back. A multi-line tell arrives as `pchat2: two` and
  `|lines`. A tell to a login that is not on gives `: ERR user 'pview1' not found.`
* **Engine logs.** An engine convention, not GGS, and not in the fixtures. Edax and its clones `tell` their
  search logs to the channel `.` plus the first 7 characters of their login (`.iEdax`, `.Rhapsod`), one
  multi-line tell or one tell per line, in their own format. The viewer takes a channel line as a log
  when the sender owns the channel, keeps it as sent, and divides the logs at each `/os: update`.
* **Kibitz echo.** `/os` does not send a kibitz line back to its sender (GameLib `VC_SEQ.C:101`). In the
  capture the other login received `/os: .1 A 1720 pchat1: kibitz to all` and
  `/os: .1 O 1720 pchat1: observers only`, and pchat1 received neither.
* **passw echo.** The ack of `passw new old` repeats both passwords:
  `: +ack:  passw zzsecret1 zzold2 => passw zzsecret1 zzold2`. The viewer masks it when it reads the line.
* **Tournaments (`/td`).** `fixtures/live-tourney.txt` is one viewer login while `/td` ran tournament 1
  (2-round round robin) and tournament 2 (2-round swiss) between `nra` and `nrb`. Replies are blocks led
  by `/td: ` with `|` lines. Errors are `/td: ERR <n>: <cmd>: <text>`: `ERR 2: st: can't issue command
  before tournament starts` (also for `rankings`), `ERR 16: rankings: tournament id does not exist` and
  `ERR 18: sr: round does not exist`. An error does not name the tournament.
  * `tell /td tourneys` answers `/td: tourneys: <n>`, a header row, then rows like
    `   1  root  /os  2   0/  2  1:48  1:00//0:30  8`: id, director, service, players, rounds played and
    total, start, clock and type. The total is padded after the `/`, so `0/  2` splits into two tokens.
    The start is an `m:ss` countdown, `now` or `done`, and a finished row ends `   Winner: nra`. The swiss
    row counts its `(bye)` filler once `/td` has added it (`#pl` 3 for two players).
  * `tell /td finger <id>` answers `/td: finger: <id>`, a line `t <login> finger: ` naming the asker, then
    `key : value` lines: `id`, `joining` (`open` even after the end), `director`, `service`, `rounds`
    (`0/2`), `style` (`rrobin` or `swiss`), `clock`, `type`, `breaks`, `players` (count first, a
    parenthesized name is the filler: `3 nrb nra (bye)`), and last a status line such as
    `round 1 begins in  1:46`, `round 2 is being played` or `tournament has finished`.
  * `tell /td st <id>` answers `/td: st: schedule for tournament <id>:`, an empty line, then
    `round   1:` lines each followed by its boards: `  1.1       nrb      nra    +0.0` is round.board,
    black, white and black's disc margin. A margin may end `:r` or `:t` (*source only*). `?` is a game
    being played and `unstarted` a later round-robin round; swiss lists only paired rounds. *Source only:*
    `  1.2       nra rests` for a bye. `sr <id> <round>` answers
    `sr: schedule for round <round> in tournament <id>:` with one round; `sp <id> <login>` answers
    `sp: schedule for player <login> in tournament <id>:` with board lines and no `round` lines.
  * `tell /td rankings <id>` answers `/td: rankings: tournament <id>` and rows best first:
    ` 1.5 ( 1  1  0) {   9.00 }      nra [0.4102]` is points, wins, draws and losses, average disc
    margin, login and tiebreak.
  * Tournament games are ordinary `/os` matches with mode `T`: `+ match .46 2121 nra 2123 nrb 8b T`, and
    `- match .46 ... T +18.00  .376`. The entry names no tournament; player 1 is black. When two
    tournaments pair the same players at once (`.44` and `.45`, then `.46` and `.47`), the pairing cannot
    tell which match is its own. `/td` learns the result after `- match`.
  * Broadcasts: every login gets `.chat /td: ` blocks (a new tournament's announcement,
    `Congratulations to nra, winner of tournament 1`). Joins, `starting round 1 of tournament 1`,
    `ending round 1 of tournament 1` with the round's board lines, and `tournament 1 is over.` go only to
    channel `.tourney`. The viewer does not join it on its own; the user can join it from the Chat tab.
