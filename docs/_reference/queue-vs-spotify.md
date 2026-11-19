---
title: Queue and Shuffle Compared with Spotify
description: An audit of every known difference between Spotifast's queue and shuffle and those of Spotify's own desktop app, with causes and possible fixes.
---

This is an audit, written on 2026-11-19 against `main` at 435aee0 (0.22.0).
It lists every difference found between how Spotifast's queue and shuffle
behave and how Spotify's own desktop app behaves, ordered by severity. It
changes no code. [The Queue's Rules](queue.md) remains the contract Spotifast
follows today; this page says where that contract, or the code behind it,
departs from Spotify.

## How this was checked

Nothing was built or run. Every finding comes from reading:

- **Spotify's desktop client**: the cached `xpui` and web player bundles
  (`~/.cache/spotify-electron/xpui/`, `webplayer/web-player.9d2e05ca.js`) and
  their English strings (`xpui/i18n/en.json`). The queue panel lives in
  `dwp-panel-section.js`; the player, queue and shuffle APIs in the web player
  bundle.
- **Soloist's documented WebSocket API**
  (developer.spotify.com/documentation/soloist/reference/websocket-api).
- **Spotifast**: `src/app.rs`, `src/ui/queue.rs`, `src/ui/collection.rs`,
  `src/ui/widgets.rs`, `src/backend.rs`, `src/api/client.rs`,
  `src/soloist/protocol.rs`, `src/soloist/mod.rs`, `src/player.rs`,
  `src/app/radio.rs`, `src/restricted_device.rs`, and the commits c278df1,
  122eb11, 5cbe154, 13c5701 and d64cebb.
- **librespot** as Spotifast pins it (crmne/librespot 3ba977f,
  `connect/src/state/*.rs`, `connect/src/spirc.rs`).
- **Upstream crmne/spotifast** pull requests and issues about the queue and
  shuffle.

Each finding says which playback path it affects:

- **S**: Soloist, Spotify's own player driven by Spotifast (the owner's path).
- **L**: the built-in librespot player ("This computer" without Soloist).
- **R**: any other Spotify Connect device, driven through the Web API.

A finding marked *needs a device check* rests on strong evidence that could
not be confirmed without running the app.

## What Spotify does

These facts about Spotify's desktop app come from its client code.

- **The queue panel** (`dwp-panel-section.js`) has three sections: *Now
  playing*; *Next in queue*, the songs queued by hand, with a **Clear queue**
  button that asks "Clear queue? You can't undo this." first; and *Next from:
  \<context\>*, a link to the playing playlist or album. That last heading
  falls back to *Next up* when there is no context name, or when the playing
  song or the next one is an autoplay recommendation; a disclosure about
  recommendations then shows. Which section a row belongs to comes from the
  player's own `provider` field on each entry (`queue`, `context`,
  `autoplay`), never from a guess. A *Recently played* tab shows the player's
  history.
- **Editing the queue** (`removeFromQueue`, `clearQueue`, `reorderQueue`,
  `insertIntoQueue` in the web player bundle) rewrites the player's
  `next_items` with its revision, on whatever device plays. Any upcoming row
  can be removed or moved, a context row included, unless the player's
  restrictions forbid it (`disallowRemovingFromContextTracksReasons`,
  `disallowReorderingInContextTracksReasons`). **Clear queue** removes only
  the hand-queued rows. Dragging a context row above a queued one makes it
  queued.
- **Clicking a queue row** calls `skipToNext({uri, uid})`, one
  `nextTrack(null, {track})`: a single skip that names the row. Nothing in
  between plays.
- **Previous** is `smartPreviousTrack`: the player restarts the song or steps
  back. Soloist's `skip_prev` and librespot (`spirc.rs`, 3 s) behave the same.
- **Shuffle is a mode of each collection, not of the app.** The shuffle
  button of a playlist or album page reads and sets that collection's own
  mode (`getShuffle(uri)` and `setShuffle(uri, mode)`, query key
  `shuffle-state`). A collection that is not playing has its mode stored on
  Spotify's side (GraphQL `centralisedStatePlayerOptions`, fields
  `shufflingContext` and `modes`). There are three modes: off, shuffle and
  **Smart Shuffle** (`context_enhancement: RECOMMENDATION`). Smart Shuffle
  adds recommendations to the queue, marked with an icon, and each can be
  removed.
- **Shuffle style** is a desktop setting: *Standard* or *Fewer repeats*.
- **Sorted or filtered views** play the same context, with
  `sorting.criteria` and `filtering.predicate` in its metadata.
- **Add to queue on a collection** takes its first 100 songs in one queue
  operation (`getContents(uri, {limit: 100})`).
- **Starting a context while songs are queued** keeps them under *Next in
  queue*.

Soloist's API exposes part of this. `get_queue` and the `queue_changed` event
return `previous` and `upcoming` entries, each with a stable `uid`, a
`source` of `context`, `queue` or `autoplay`, and the item. `playback_state`
carries `options.shuffle`, `options.repeat`, `options.modes`
(`context_enhancement`: `NONE` or `RECOMMENDATION`) and `available_actions`.
Its commands are `play` (optional `uri`), `pause`, `skip_next`, `skip_prev`,
`seek`, `set_volume`, `set_shuffle`, `set_repeat_context`,
`set_repeat_track`, `add_to_queue` (tracks only), `activate` and
`deactivate`. It has nothing to remove, reorder, clear or insert into the
queue, and nothing to skip to a given row.

## Summary

| # | Difference | Paths | Severity |
|---|---|---|---|
| 1 | Shuffle is one app-wide switch, not a mode of each collection | S L R | High |
| 2 | The shuffle button shows Spotifast's wish, not the player's state | S R | High |
| 3 | A shuffled start turns shuffle on in the old collection first, and can deadlock | S R | High |
| 4 | Clicking a queue row sends one Next per row above it | S R | High |
| 5 | *Playing next* and *Next up* are guessed; Soloist's `source` is thrown away | S R L | High |
| 6 | No queue editing at all while Soloist plays | S R | High |
| 7 | Rows past the first few of a Soloist queue arrive without names | S | High |
| 8 | With Shuffle on, a lone song skips Soloist's `play` and loses Autoplay | S | Medium |
| 9 | Smart Shuffle is invisible, and cannot be chosen | S R | Medium |
| 10 | A shuffled Play draws its first song only from the rows loaded so far | S L R | Medium |
| 11 | Sorted views, artist Popular, Home lists and Recent play as plain lists | S L R | Medium |
| 12 | Restarting Spotifast loses the queue on Soloist | S | Medium |
| 13 | Autoplay on Soloist: no switch, and Spotifast's own radio fallback | S R | Medium |
| 14 | Built-in player: editing rewrites the whole queue; *Next up* rows cannot be edited | L | Medium |
| 15 | Adding a collection sends one Web API request per song | S R | Medium |
| 16 | Section headings differ, and *Next from* is not a link | S L R | Low |
| 17 | **Clear queue** asks for no confirmation | L | Low |
| 18 | *Recent* is not the player's history | S L R | Low |
| 19 | The built-in player's queue is read back through the Web API | L | Low |
| 20 | The padding filter can hide real rows | S L R | Low |
| 21 | librespot forgets the play history on every shuffle toggle | L | Low |
| 22 | No shuffle style choice | S L R | Low |
| 23 | Features Spotify's desktop app does not have | S L R | Low |

## High severity

### 1. Shuffle is one app-wide switch, not a mode of each collection

- **Spotify:** each playlist, album and Liked Songs keeps its own shuffle
  mode, stored on Spotify's side. Starting a collection plays it in its own
  mode. The page's shuffle button shows and changes that page's mode,
  without touching what is playing when another collection plays.
- **Spotifast:** one flag, `shuffle_wanted`, applies to every start
  (`src/app.rs:10539-10552`). The page's shuffle button shows that flag
  (`playing_context_shuffle`, `src/app.rs:2263`) and toggles the current
  playback (`src/ui/collection.rs:525-550` → `set_shuffle`,
  `src/app.rs:11294`). The [queue rules](queue.md) state this on purpose:
  "toggling Shuffle changes that playback's mode without switching to the
  page's collection".
- **Effect:** an album started after a shuffled playlist plays shuffled, and
  the reverse (issue crmne/spotifast#1187). Pressing Shuffle on an album's
  page while a playlist plays reshuffles the playlist instead of setting the
  album. On Soloist the two models clash: Spotify's player applies the
  started collection's own mode. Soloist's events in crmne/spotifast#1245
  show Liked Songs started with shuffle on coming up in **Smart Shuffle**
  (`options_changed {"modes":{"context_enhancement":"RECOMMENDATION"},"shuffle":true}`),
  which matches crmne/spotifast#664, "playing songs not from my Liked Songs".
- **Cause:** Spotifast models shuffle as a global preference, as Spotify's
  apps did before per-collection play modes.
- **Fix:** remember a mode per collection URI (crmne/spotifast#1119 does it
  per kind, playlist or album; #1187 asks for each collection). Start a
  collection in its own mode. Make the page button act on its own
  collection and leave the playing one alone. Reading Spotify's stored mode
  (`centralisedStatePlayerOptions`) needs a GraphQL read that the session
  may allow; Spotifast already makes such reads (`src/session_reads.rs`).
- **Fixable:** yes, as a local memory. Sharing the mode with Spotify's apps
  needs a device check of that GraphQL read.

### 2. The shuffle button shows Spotifast's wish, not the player's state

- **Spotify:** the shuffle button shows the player's state.
- **Spotifast:** for another device, Soloist included, the shown state is
  `shuffle_wanted` (`src/app.rs:2514`, also `2413` and `2558`), not
  `remote.state.shuffle_state`, which Soloist reports
  (`src/soloist/protocol.rs:436`). The device's value replaces the flag only
  when it flips between two consecutive polls, and only more than 5 s after
  Spotifast's own last change (`src/app.rs:8186-8205`; local equivalent
  `src/app.rs:3869-3876`). **Shuffle** in the player bar toggles from that
  flag (`src/app.rs:12903-12908`).
- **Effect:** after a start in which Spotify applies the collection's own
  mode (finding 1), or any change that lands inside the 5 s window or
  between two polls, the button and the queue disagree with what plays.
  Pressing **Shuffle** then sends the state the device already has: nothing
  happens, and it takes a second press. Every later play reapplies the wrong
  flag.
- **Cause:** an optimistic flag with no expiry, and change detection by
  flips instead of by comparison with the device.
- **Fix:** show the device's reported state whenever it is fresh. Keep
  `shuffle_wanted` only as a short optimistic hold after a toggle, as
  `OPTIMISTIC_HOLD` does for play and pause, and take the device's value
  after it.
- **Fixable:** yes. *Needs a device check* for how often it bites.

### 3. A shuffled start turns shuffle on in the old collection first, and can deadlock

- **Spotify:** one play command carries the new collection's options
  (`player_options_override.shuffling_context`).
- **Spotifast:** with Shuffle on, every start on another device, Soloist
  included and a clicked row included, is `ApiRequest::ShufflePlay`
  (`src/app.rs:10654-10658`): `PUT /me/player/shuffle?state=true`, then
  `PUT /me/player/play`, and the play is skipped if the first call fails
  (`src/backend.rs:6963-6972`). Soloist's dispatch does not handle
  `ShufflePlay` (`src/soloist/protocol.rs:194`), so both go through the Web
  API.
- **Effect:** the shuffle call applies to what is loaded now. If that
  forbids shuffle, as an autoplay radio or a restored session does, Spotify
  answers 403 "Restriction violated" and **nothing can be started, even
  after a restart** (crmne/spotifast#1307, #1346). Otherwise the old
  collection is reshuffled for a moment, and its queue can flash on screen.
- **Cause:** two independent requests where Spotify sends one. Upstream
  fixed the deadlock in df75525 (crmne/spotifast#1348, released in 0.17.2):
  when shuffle is refused, it plays first and turns shuffle on after. That
  commit is **not in this fork**; 122eb11 and 5cbe154 were taken from the
  same upstream range, but df75525 was not.
- **Fix:** cherry-pick df75525 now. Better: send the play over Spotify
  Connect with `player_options_override.shuffling_context`, as Spotify's
  apps do. Spotifast already sends Connect `play` commands through the
  signed-in session for restricted devices (`play_command` and `command`,
  `src/restricted_device.rs:114-182`), with an empty
  `player_options_override` today (line 143).
- **Fixable:** yes. The Connect route needs the session signed in to the
  same account (Settings → Spotify Soloist → Spotify access).

### 4. Clicking a queue row sends one Next per row above it

- **Spotify:** one skip that names the row by `uid`. Nothing in between plays.
- **Spotifast:** on another device, Soloist included, `play_queue_item` sends
  one `Next` per row down to the chosen one (`src/app.rs:7228-7241`). The
  built-in player was fixed in 122eb11: it removes the rows above from the
  engine's queue, then skips once.
- **Effect:** each song in between starts briefly. Soloist's state answers
  in between show those songs as playing, the rows flicker, and the songs
  passed through enter the player's history. Ten rows down means ten Nexts
  racing to the device (crmne/spotifast#1316).
- **Cause:** neither the Web API nor Soloist's documented API can skip to a
  given row.
- **Fix:** send a Connect `skip_next` naming `{uri, uid}`, as Spotify's apps
  do. Soloist's `get_queue` already gives each row's `uid`, which
  `src/soloist/protocol.rs:160-166` drops today. Until then, upstream
  crmne/spotifast#1353 (open) serializes the skips and holds the chosen row
  on screen.
- **Fixable:** yes, through the session's Connect route. *Needs a device
  check* that Soloist accepts a Connect `skip_next` with a track; Spotify's
  own apps use it on every Connect device.

### 5. *Playing next* and *Next up* are guessed; Soloist's `source` is thrown away

- **Spotify:** a row's section comes from the player's provider: `queue`
  rows go under *Next in queue*, `context` rows under *Next from*,
  `autoplay` rows under *Next up*.
- **Spotifast:** the split counts how many leading rows match, by URI, the
  songs queued **in this Spotifast session** (`manual_queue`,
  `end_of_queued_rows`, `src/app.rs:11958-11974`; `queued_rows_len`,
  `src/app.rs:7247`; `src/ui/queue.rs:455`). Soloist reports each row's
  `source` and `uid`, but `src/soloist/protocol.rs:160-166` keeps only the
  item.
- **Effect:** songs queued from the phone, from Spotify's desktop app, or
  before Spotifast was restarted, show under *Next up*. If such a song is
  first, every row Spotifast queued itself falls under *Next up* too, and
  **Clear queue** and **Remove from queue** disappear with *Playing next*.
  A context row that shares a URI with a queued song can be counted on the
  wrong side. Autoplay recommendations look like the playlist's own songs.
- **Cause:** the Web API's `GET /me/player/queue` has no provider, and the
  Soloist path was mapped to the same model.
- **Fix:** keep `source` and `uid` in the queue model and split by them
  whenever the player gives them: always on Soloist. The built-in player
  knows each row's provider too (librespot's `ProvidedTrack`, see finding
  19). Keep the guess only for Web API devices.
- **Fixable:** yes for S and L. Not for R: the Web API does not say.

### 6. No queue editing at all while Soloist plays

- **Spotify:** clear, remove any row, reorder any row and insert at a
  position, on every device.
- **Spotifast:** **Clear queue**, **Remove from queue**, drag-to-reorder,
  drop-at-position and **Play next** are offered only while the built-in
  player is active (`queue_locally_reorderable`, `src/app.rs:7271-7284`;
  `clear_queue`, `src/app.rs:7352-7355`; **Play next**,
  `src/app.rs:12917-12923` and `src/ui/widgets.rs:744-747`). Elsewhere a drop
  just appends (`src/ui/queue.rs:331-341`).
- **Effect:** on Soloist the queue can only grow. A song added by mistake
  stays until it plays (crmne/spotifast#1203, #1253, #555).
- **Cause:** Soloist's documented API has no command for any of these, and
  neither has the Web API. The [Soloist guide](soloist.md) says so.
- **Fix:** Spotify's apps change another device's queue with a Connect
  `set_queue` command: the next and previous tracks with their `uid`s and
  the queue revision (librespot handles it in `handle_set_queue`). Spotifast
  could send it through the session's Connect route, from Soloist's
  `get_queue` rows. The [Soloist guide](soloist.md) already names this route
  as unverified.
- **Fixable:** maybe. *Needs a device check*: the queue revision comes from
  Spotify's Connect state, not from Soloist's API, and concurrent changes
  must not be lost. Without it, not fixable with the documented APIs.

### 7. Rows past the first few of a Soloist queue arrive without names

- **Spotify:** every queue row has its title, artist, cover and length.
- **Spotifast:** `item()` (`src/soloist/protocol.rs:342`) reads names only
  from the entry's `decorations`. Soloist decorates only the first entries
  of a full `get_queue` and sends the rest as bare URIs.
- **Effect:** a long *Next up* shows rows without title, artist or length
  (crmne/spotifast#1277, open). An entry without `entity_type` is dropped
  altogether, which shifts row indexes.
- **Cause:** Soloist's API decorates only the head of the queue.
- **Fix:** fill the bare rows in one batched metadata read, as
  crmne/spotifast#1277 does over the session, or through the Web API's
  several-tracks read. Keep bare rows as placeholders rather than dropping
  them.
- **Fixable:** yes. Reported upstream; *needs a device check* on this
  Soloist build.

## Medium severity

### 8. With Shuffle on, a lone song skips Soloist's `play` and loses Autoplay

- **Spotify:** a song played on its own is its own context, and autoplay
  follows it.
- **Spotifast:** 13c5701 sends a lone song through Soloist's
  `play {uri}` (`src/soloist/protocol.rs:279-283`), but only for
  `ApiRequest::Remote`. With Shuffle on, the same start is
  `ApiRequest::ShufflePlay` (`src/app.rs:10654`), which Soloist's dispatch
  passes to the Web API (`src/soloist/protocol.rs:194`) as a one-song `uris`
  list.
- **Effect:** with Shuffle on, a song started from search plays with no
  context. Soloist stops when it ends (crmne/spotifast#1320); only
  Spotifast's radio fallback (finding 13) carries on, later and as a plain
  list.
- **Cause:** the lone-song route was added for one request type only.
- **Fix:** route a lone song through Soloist's `play` whatever the shuffle
  flag; shuffling a single song means nothing.
- **Fixable:** yes.

### 9. Smart Shuffle is invisible, and cannot be chosen

- **Spotify:** shuffle cycles off, shuffle, Smart Shuffle. Smart Shuffle
  mixes recommendations into the queue, each marked and removable.
- **Spotifast:** shuffle is on or off. Soloist's `options.modes` is not read
  (`playback`, `src/soloist/protocol.rs:396-438`), so Smart Shuffle shows as plain
  shuffle, and its recommendations as ordinary rows.
- **Effect:** unknown songs play in a shuffled playlist with nothing to
  explain them (crmne/spotifast#664). Spotify can switch a collection to
  Smart Shuffle by itself (finding 1). Turning shuffle off in Spotifast also
  ends Smart Shuffle, as the last step of Spotify's own cycle does.
- **Cause:** the Web API and Soloist have no Smart Shuffle command;
  crmne/spotifast#133 and #500 were closed as won't fix.
- **Fix:** read `options.modes.context_enhancement` and show a third state
  on the button; mark rows Soloist reports as recommendations. Choosing
  Smart Shuffle would need an undocumented Connect command.
- **Fixable:** showing it, yes; choosing it, no with the documented APIs.

### 10. A shuffled Play draws its first song only from the rows loaded so far

- **Spotify:** the player picks the first song among the whole collection.
- **Spotifast:** `shuffle_start` (`src/app.rs:10454-10465`) prefers
  `random_track_in` (`src/app.rs:10398-10401`), which draws from the rows
  Spotifast holds (`context_track_uris`, `src/app.rs:10366-10396`): for a
  playlist, its loaded pages. It falls back to a random position in the
  known length only when no row is loaded (d64cebb).
- **Effect:** in a long playlist whose page has loaded its first rows only,
  every shuffled Play starts among those rows. That matches
  crmne/spotifast#1300, "Shuffle only chooses from the first songs of a long
  playlist", still open; its cause is not established, and this is one
  part.
- **Cause:** the Web API starts a shuffled collection at its first song when
  no offset is given, so Spotifast picks the offset itself.
- **Fix:** when the collection's length is known and its rows are not all
  loaded, pick a random position across the whole length. A Connect `play`
  with `shuffling_context` and no `skip_to` would let the player choose, as
  Spotify's apps do.
- **Fixable:** yes.

### 11. Sorted views, artist Popular, Home lists and Recent play as plain lists

- **Spotify:** a sorted or filtered view plays its collection with
  `sorting.criteria` or `filtering.predicate`. A Popular song on an artist's
  page plays the artist.
- **Spotifast:** a sorted or filtered view hands over its shown rows as a
  list of at most 500 songs (`src/app.rs:12778-12820`,
  `src/ui/collection.rs:993-1005`). An artist's Popular rows
  (`src/ui/artist.rs:521`), Home's song rows (`src/ui/home.rs:1854`) and
  *Recent* rows also play as plain lists.
- **Effect:** no *Next from*; *Playing from* shows only what Spotifast
  assumes. Past 500 songs a sorted view stops. Autoplay does not follow
  natively: Spotifast detects the end and plays a radio (finding 13). An
  artist's Popular list ends after its ten songs, where Spotify carries on
  through the artist; only that radio fallback follows. The
  [queue rules](queue.md) say "A playlist, album, artist or
  Liked Songs started from a row … plays as that context"; the artist's
  Popular rows do not.
- **Cause:** the Web API's play request has no context metadata, and its
  `offset` works only for albums and playlists.
- **Fix:** send a Connect `play` with the context's metadata and `skip_to`,
  as Spotify's apps do, through the session. Play an artist's Popular row as
  `spotify:artist:…` with `skip_to` the song.
- **Fixable:** partly: through the session's Connect route, *needs a device
  check*. Not through the Web API.

### 12. Restarting Spotifast loses the queue on Soloist

- **Spotify:** the queue survives a restart of the app.
- **Spotifast:** the saved queue (`resume_queue`) is queued again only when
  the built-in player resumes (`now.local`, `src/app.rs:4299-4318`).
  Closing Spotifast stops Soloist.
- **Effect:** after a restart with Soloist, the queued songs are gone, and
  the queue rule "Closing the app keeps the queue" does not hold.
- **Cause:** the restore was written for the built-in player.
- **Fix:** when the remembered song resumes on Soloist, add the saved songs
  again with Soloist's `add_to_queue`.
- **Fixable:** yes.

### 13. Autoplay on Soloist: no switch, and Spotifast's own radio fallback

- **Spotify:** Autoplay is a setting. When a collection ends, Spotify's
  recommendations follow without a gap and are listed ahead under *Next up*.
- **Spotifast:** with Soloist, the Autoplay switch is hidden with the other
  built-in settings (`src/ui/settings.rs:698-700`). Soloist follows the
  account's own Autoplay setting after a collection (tested in
  crmne/spotifast#1320). After a plain list
  (finding 11), Spotifast waits for the device to stand stopped at the
  start of the last song (`remote_list_end`, `src/app.rs:15296`), then plays
  that song's radio as another plain list (`src/app/radio.rs:197-227`),
  gated by the hidden `settings.autoplay` (`src/app.rs:8169`).
- **Effect:** a gap of silence before the radio. The radio is not listed
  ahead, and it ignores the account's Autoplay choice. When it ends, the
  same detection starts again.
- **Cause:** Soloist's API has no Autoplay setting, and a plain list gives
  the player nothing to follow.
- **Fix:** mostly fixed by playing contexts (findings 8 and 11). Show the
  switch with Soloist as "Spotifast's own radio after a list", or follow
  the account setting where it can be read.
- **Fixable:** partly.

### 14. Built-in player: editing rewrites the whole queue; *Next up* rows cannot be edited

- **Spotify:** remove or move any upcoming row; other rows are untouched.
- **Spotifast:** a move, an insert, **Play next** and **Remove from queue**
  clear the engine's queue and add `manual_queue` back
  (`resync_local_queue`, `src/app.rs:11812-11828`). Only *Playing next*
  rows can be removed or moved (`src/app.rs:7282-7284`), and *Next up* is
  never a drop target (`src/ui/queue.rs:565-578`).
- **Effect:** songs another device queued to this computer vanish at the
  first edit. A playlist song cannot be taken out of *Next up* or moved
  forward.
- **Cause:** librespot's Spirc API had no single-row edit; the fork now has
  `remove_next_track` (used by 122eb11).
- **Fix:** remove rows with `remove_next_track`, for *Next up* rows too
  (crmne/spotifast#908, draft). Moves and inserts would need a `set_queue`
  call in the librespot fork.
- **Fixable:** yes.

### 15. Adding a collection sends one Web API request per song

- **Spotify:** one queue operation adds a collection's first 100 songs.
- **Spotifast:** on Soloist and other devices, `add_many_to_queue` sends one
  `POST /me/player/queue` per song, one after another, up to 500
  (`src/api/client.rs:866-878`). Soloist's own `add_to_queue` is not used
  (`src/soloist/protocol.rs:192-194`).
- **Effect:** a large playlist takes a long time to appear, spends the Web
  API quota and can hit rate limits; additions wait behind it.
- **Cause:** additions were kept on the serialized Web API writer so they
  cannot overtake one another.
- **Fix:** on Soloist, send additions through its local `add_to_queue`,
  serialized the same way, with no Web API quota spent.
- **Fixable:** yes.

## Low severity

### 16. Section headings differ, and *Next from* is not a link

- **Spotify:** *Now playing*, *Next in queue*, *Next from: \<name\>* as a
  link; *Next up* when what follows is autoplay, with a note about
  recommendations.
- **Spotifast:** *Playing from \<name\>* above the song, then *Now playing*,
  *Playing next* and *Next up* (`src/ui/queue.rs:377`, `397`, `465`, `572`).
  No heading changes when autoplay takes over.
- **Fix:** use Spotify's headings, a *Next from* link to the context, and
  *Next up* from the first autoplay row once finding 5 gives the provider.
- **Fixable:** yes.

### 17. **Clear queue** asks for no confirmation

- **Spotify:** "Clear queue? You can't undo this."
- **Spotifast:** clears at once and shows a toast (`src/ui/queue.rs:294`,
  `src/app.rs:7352`).
- **Fixable:** yes.

### 18. *Recent* is not the player's history

- **Spotify:** the queue panel's history is the player's own previous
  tracks, skipped ones included.
- **Spotifast:** *Recent* merges Spotify's recently played list with
  Spotifast's local history (`src/ui/queue.rs:73`). Soloist's `previous`
  entries are not read (`src/soloist/protocol.rs:160-166`).
- **Effect:** songs skipped after a few seconds, and the order Previous will
  follow, are not shown.
- **Fixable:** yes on Soloist, from `previous`.

### 19. The built-in player's queue is read back through the Web API

- **Spotify:** the panel reads the player it drives.
- **Spotifast:** even for the built-in player, the queue comes from
  `GET /me/player/queue` (`src/backend.rs:6549-6552`,
  `src/app.rs:7128-7148`): about 20 rows, padded (finding 20), and seconds
  behind, which the stale-answer checks (`src/app.rs:7501`) work around.
- **Fix:** read librespot's own next tracks, with providers and `uid`s.
  librespot can emit them (`emit_set_queue_events`, librespot
  `connect/src/spirc.rs:998`).
- **Fixable:** yes.

### 20. The padding filter can hide real rows

- **Spotify:** shows what the player will play.
- **Spotifast:** with Repeat off, rows from where the list comes back round
  to the playing song are cut (`drop_wraparound`, `src/app.rs:11981-12007`,
  5cbe154). It runs for every source, Soloist included, although the
  padding is a Web API artifact.
- **Effect:** a list whose order genuinely repeats, such as A B A B, loses
  rows. Near the end of an album, rows that will not play can stay, as the
  queue rules note.
- **Fix:** skip it where the player gives providers (Soloist, built-in).
- **Fixable:** yes.

### 21. librespot forgets the play history on every shuffle toggle

- **Spotify:** toggling shuffle keeps the current song and the queue; the
  history stays (*needs a device check*).
- **Spotifast (built-in player):** librespot's `shuffle()` clears the
  previous tracks (`connect/src/state/options.rs:84-86`), called from
  `PlayerCommand::Shuffle` (`src/player.rs:892`).
- **Effect:** after a toggle, Previous cannot go back past the current song.
- **Fix:** keep `prev_tracks` across a reshuffle in the librespot fork.
- **Fixable:** yes, in the fork.

### 22. No shuffle style choice

- **Spotify:** *Standard* or *Fewer repeats*.
- **Spotifast:** none. Soloist, being Spotify's player, may follow the
  account's choice; the built-in player shuffles uniformly.
- **Fixable:** no setting is exposed to Spotifast; low value.

### 23. Features Spotify's desktop app does not have

These differ from Spotify, but on purpose, and harm nothing:

- **Play next** (front of the queue) exists only in Spotifast, and only on
  the built-in player.
- **Add to queue** with nothing playing starts the choice; Spotify offers no
  **Add to queue** then.
- Adding a collection takes up to 500 songs; Spotify takes 100.

## What already matches

- Starting a collection keeps the songs queued by hand (rule 6).
- **Add to queue** goes after the songs already queued and before the
  collection; queued songs play first.
- With Shuffle on, a clicked row plays first and the rest is shuffled
  (`local_load`, `src/app.rs:15253`, and Spotify's player on Soloist).
- Toggling shuffle mid-collection keeps the current song and the queued
  songs in order; turning it off continues in order from the current song.
  The player does this: Soloist is Spotify's own, and librespot's
  `handle_shuffle` does the same.
- **Previous** restarts after 3 s and otherwise steps back on all three
  paths (Soloist's `skip_prev`, librespot's `handle_prev`,
  `RESTART_BEFORE_PREVIOUS` for the remembered song).
- Choosing another song by hand turns Repeat One into Repeat All
  (`leave_repeat_one`, `src/app.rs:11365`).
- A lone song on an active Soloist plays as its own context, so Autoplay
  follows it (13c5701), with Shuffle off (see finding 8).
- On the built-in player, a clicked queue row starts without the rows above
  it playing (122eb11).

## Fix order suggested

1. Cheap and contained: cherry-pick df75525 (3); a lone song through
   Soloist's `play` with Shuffle on (8); show the device's shuffle state (2);
   keep Soloist's `source` and `uid` (5), which unlocks 16, 18 and 20;
   restore the queue on Soloist (12); add through Soloist (15).
2. A model change: a shuffle mode per collection, and the page button on
   its own collection (1); first song across the whole length (10).
3. A spike on Spotify Connect commands sent through the session to Soloist:
   `skip_next` naming a row (4), `set_queue` (6), `play` with options and
   context metadata (3, 10, 11). If Soloist accepts them, most of the
   remaining gaps close.
