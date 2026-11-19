---
title: The Queue's Rules
description: What the queue shows, what Add to queue does, and what the app promises about both.
nav_order: 2
---

The queue is the list of what plays next. It has up to three parts, as in
Spotify's own apps. On top, under **Next in queue**, are the songs queued
by hand. Below them, under **Next from**, followed by the name of the
playing playlist or album, which opens when clicked, are the songs that
come next in it; when that name is not known the heading reads **Next
up**. Last, under **Next up**, with a line saying they are recommendations,
are the songs Spotify's autoplay adds once the playlist or album ends.
Queued songs always play first.

Which part a row belongs to comes from the player whenever it says.
Soloist, Spotify's own player, reports where each row comes from, as it
does to Spotify's apps: a song queued from the phone or from Spotify's
desktop app is under **Next in queue** too, and a playlist row that
happens to be a song you queued stays the playlist's. The Web API, which
other devices and the built-in player's queue are read through, does not
say: there the songs queued in this Spotifast session make **Next in
queue**, and the rest of the queue sits under **Next from**. Rows Soloist
sends without names keep their place and are named from one read of their
details.

Above both, **Playing from** names where the playing song came from: the
playlist, album, artist, or podcast, which opens when clicked, Liked
Songs, or a radio named after the song, playlist, album, or artist it is
based on, which opens the radio's page. The line shows only while a song is
playing from somewhere Spotify reports.

These are the rules the app follows. The queue tests in `src/app.rs`
check every one of them.

Starting and resuming are separate actions. With Shuffle off, a playlist's
**Play** button starts at its first available song in the selected order.
Double-clicking a row starts there, including with Shuffle on. **Play** in
the player bar resumes the current song at its paused position.

With Shuffle on, a playlist's, album's, podcast's or Liked Songs' **Play**
starts at a random song, wherever it is pressed: on its page, a Home tile or
card, or the library sidebar. When its songs are not loaded yet and another
device such as Soloist plays, Spotifast first reads how many songs it holds,
then starts at a random position within it; the play shows as starting
meanwhile, and the length is remembered for the next time. If that read
fails, the play still starts, and Spotify chooses where. An artist has no
position to start at, so Spotify shuffles it on its own.

The Shuffle buttons show what the playing device reports. A change made in
Spotifast shows at once and holds for a few seconds while the device
catches up; after that the device's own state shows again, whoever
changed it, and the next play follows it. Spotify's player applies a
collection's own mode when it starts it, so the button never stays on a
wish the device does not share, and one press always changes the mode.

Since 0.10.0, the Shuffle button on a collection page changes the
global playback mode without starting that collection. It can be selected
before a playback device is active; the next **Play** uses the selection.
While another collection plays, toggling Shuffle changes that playback's mode
without switching to the page's collection. The queue then reflects the new
play order.

Since 0.8.0, starting a playlist in its original order explicitly
names its first available song from the loaded prefix. If that prefix is not
loaded, it requests playlist position zero. A page loaded from the middle
never becomes the beginning. This keeps the full Spotify playlist context;
the app does not replace it with a shortened list of loaded songs. A request
waiting for local playback to reconnect keeps the song chosen at the click.

Sorted and filtered views omit unavailable songs, and local files the local
files folder does not hold, from their playback requests. The displayed rows keep their positions, and selecting a
repeated song starts that occurrence. In a playlist's original order, a song
that appears more than once among the loaded rows starts at the chosen copy's
playlist position, provided the rows load from the beginning and no missing
or local entry comes before it; otherwise it starts by name, which plays the
first copy. A song started from a row marks only that row as playing and
loading; other copies in the same list look like ordinary rows until another
song plays. Another playlist that also holds the playing song shows it as an
ordinary row, since the song is not playing from there. A filtered playlist or
Liked Songs view
plays its matching songs in displayed order, including duplicates. An empty or entirely
unplayable view disables Play instead of starting the unfiltered context.

With Autoplay on, what plays after the last song comes from Spotify's
recommendations for what was playing, as in Spotify's own apps. A playlist,
album, artist or Liked Songs started from a row, with Shuffle on or off,
plays as that context at the chosen song, so Spotify carries on from the
whole context when it ends. A song started on its own, as a search result
is, plays as its own context, and Spotify carries on from that song; this
holds for the built-in player and for Soloist, with Shuffle on or off,
since shuffling one song changes nothing. A sorted or filtered view
plays its shown rows as a plain list, which leaves the player no context to
carry on from. Since 0.22.0, when such a list ends, with any songs queued
behind it, Spotifast carries on from the last song that played: on this
computer's built-in player with Spotify's recommendations for it, and on
another device, Soloist included, with that song's radio (rule 13).

Since 0.9.0, starting another song on this computer keeps the
selected Repeat mode, with Shuffle on or off. Loading a playlist no longer
silently disables repeat in the playback engine.

Since 0.9.0, silencing the old song during a local track change
keeps the decoder paced while the replacement loads. Cached audio no longer
races to the end and causes an unwanted extra skip during that handoff.

Since 0.9.0, an unexpected local playback disconnect retains the
engine's in-memory playback state before closing the session. Reconnection
restores the same song and position, paused or playing, with its playlist
context, exact shuffle order, manually queued songs (including duplicates),
repeat settings, and pending context pages. This also works when the current
song came from the manual queue rather than the playlist. Playback started
on the replacement engine takes precedence over recovery.

Since 0.9.0, switching from another Connect device back to this
computer transfers that device's current song, position, and queue, including
manually queued copies. A paused session stays paused. The handoff itself does
not consume a queue row or restore an older queue saved on this computer.

1. **The list shows the play order.** The top row plays next, followed by the
   rows below it.

   The Web API pads a short queue by going round it again, back to the
   playing song, even with Repeat off. With Repeat off, Spotifast leaves
   out the rows from where the list comes back round, so a song started on
   its own, as a Recent row starts it, is not listed again under itself.
   Near the end of an album, Spotify may go round to the album's first song
   without showing where the album ends, so a few rows that will not play
   can stay. Soloist lists only what it will play, so its queue is shown
   whole, even a list whose order really repeats.

2. **Add to queue adds a song to your part of the queue.** It goes after
   the songs you queued earlier and before the playlist's songs. Queue
   the same song twice and it plays twice. A double-click only counts
   once.

   **Play next** puts a song, or a selection of songs in displayed order,
   at the front of **Next in queue**, before the songs already queued. It
   leaves the current song playing and keeps repeated songs as separate
   entries. Right-click a song or selection to find it beside **Add to queue**.
   It is enabled only while this computer is the active player, including
   when paused. Spotify does not let Spotifast insert at the front of another
   device's queue.

   A local file from a playlist queues like any other song, but only while
   this computer is the active player and the local files folder holds it:
   Spotify's queue refuses local files, so this computer's player queues
   them itself. Otherwise **Add to queue** and **Play next** are disabled
   for it, and a selection queues its other songs.

   Since 0.9.0, an album's **Add to queue** adds its playable songs
   in album order, including repeated songs. This also works for singles and
   EPs. A fully loaded album appears immediately. Otherwise, a loading notice
   appears while all its track pages are fetched, then its songs are appended
   together after the songs already queued. A failed fetch adds no partial
   album. Clear queue and sign-out cancel an album still being fetched;
   changing playback devices asks you to add it again on the new device.

   Since 0.22.0, a playlist, Liked Songs and an artist have **Add to queue**
   too, wherever their menu opens: a right-click in Your Library, on a card
   or a search result, or the **…** button on their page. A playlist and
   Liked Songs add their playable songs in their own order, repeats
   included; an artist adds its popular songs, in the order its page shows
   them. Spotify's own apps offer this for albums, playlists and Liked Songs,
   not for artists or podcasts. They load and append like an album: at once
   when every song is on screen, otherwise together once every page is read,
   with nothing added if a read fails. One addition takes at most the first
   500 songs, as many as a play hands the player; Spotify's own apps take
   the first 100. A local file joins only where a single one would.

   With nothing playing or paused on any device, and no song to resume,
   there is no queue to add to; Spotify's own apps offer no **Add to queue**
   then. Since 0.22.0, **Add to queue** starts what was chosen instead: a song
   plays on its own, so Autoplay follows it, and a playlist, album, Liked
   Songs or artist plays from its start, so it carries on as itself.

   On Soloist, songs join the queue through Soloist's own queue, one batch
   at a time and in order, with no Web API request. Episodes, which
   Soloist's queue refuses, go through the Web API.

   On another device, a rate limit delays additions instead of dropping them.
   Spotifast retries automatically after Spotify's requested wait and keeps
   later songs behind the album. Pending rows retain their known titles and
   durations. If Spotify permanently rejects an addition, only the rejected
   song and any unsent remainder of its album disappear; accepted songs stay.

3. **When a song starts, its row leaves the queue.** It doesn't matter
   how it started: the song before it ended, you pressed Next, you
   clicked its queue row, or another device skipped to it. Only that
   occurrence leaves: another copy you queued remains until its own turn.

4. **Next removes the top row right away.** The app doesn't wait for
   Spotify to confirm it.

5. **Playing a row from the queue skips to it.** The rows above it are
   skipped and removed, as if you had pressed Next down to it. The rows
   below it stay, and the playlist keeps going afterwards. On this
   computer the rows above leave the queue without playing, so only the
   chosen song loads and no song in between is heard or named. Previous
   then returns to the song that was playing, not to a skipped row. Another
   device, Soloist included, cannot have a song taken out of its queue, so
   it steps down to the row with one skip per row.

6. **Starting a playlist or album keeps your songs.** The rows underneath
   change to the new collection; your songs stay under **Next in queue**
   and play before the collection continues. If you queue an album and then
   start it from the Library, its first song plays now and the separately
   queued album still follows. **Clear queue** removes those manual copies
   while keeping the playing album's remaining songs.

7. **Clear only removes your songs.** The trash button sits beside the
   *Next in queue* heading and empties that section; the playlist's rows
   below stay. It only shows while this computer is the player, because
   that is the only queue the app can actually clear.

8. **Changes appear immediately.** Spotifast updates the queue before Spotify
   confirms the change. For local playback, it updates its own player directly.
   Toggling shuffle rechecks the queue so the new playback order appears
   promptly without waiting for the song to finish.

9. **Closing the app keeps the queue.** Spotifast saves it locally. When you
   resume the last song, it restores your queued songs and playlist position,
   and the playlist carries on after that song, even when it was paused or
   playing from a sorted view when you closed the app. Closing Spotifast
   stops Soloist, which forgets its queue; when the last song resumes on
   Soloist, Spotifast queues the saved songs there again, in one batch and
   in order, except local files, which Soloist's queue does not take.

   A song the playlist no longer holds, such as a recommendation that followed
   its end, plays on its own at its saved position, followed by your queued
   songs and, with Autoplay on, Spotify's recommendations for that song.

10. **Old answers from Spotify are ignored.** Queue responses can be a few
    seconds late. Spotifast ignores stale responses and asks again. Your
    changes stay visible while it waits for confirmation.

Since 0.8.0, selecting several playlist rows and choosing
**Add to queue** preserves repeated occurrences in their selected order.
For example, selecting B, C, B adds all three rows. A repeated click still
counts once, and the notification reports only the rows actually added.

11. **Dragging within *Next in queue* reorders it, only on this computer.**
    Neither the Web API nor librespot can reorder or insert into a live
    queue; the only way to change one is to clear it and re-add its songs
    in the new order, which reaches nothing but the engine actually playing
    them. So dropping a song, dragged from elsewhere, at a position in
    *Next in queue* inserts it there, and dragging a row already in
    *Next in queue* elsewhere in the same section moves it, only while this
    computer is the active player. Otherwise every drop still just adds to
    the end, exactly like **Add to queue**. *Next from* and *Next up* are
    never a drop target: they play from the current context, not from a
    list Spotifast can rewrite. While *Next in queue* is empty, drop the
    song on the player bar's Queue button instead.

12. **A song you queued can be removed on its own, only on this
    computer.** Right-click a row under *Next in queue*, or open its **…**
    menu, and choose **Remove from queue**. Only that row leaves: other
    copies of the song stay, and so do the playlist's rows. Like a move, this
    rewrites the queue by clearing it and re-adding the rest, so it is
    offered only while this computer is the active player. The playlist's
    rows have no such entry; they play from the current context.

13. **When the queue runs out, playback carries on.** Your songs play
    first, then the playlist or album carries on where it was, under
    **Next from**. When it has ended as well, Repeat off and Autoplay on,
    what follows comes from Spotify's recommendations for the last song that
    played, a queued one included, as in Spotify's own apps. A playlist,
    album, artist, Liked Songs or a song on its own carries on by itself;
    on Soloist its recommendations are listed ahead, under **Next up**.
    A plain list, as a sorted or filtered view plays, does not: on this
    computer's player Spotifast then loads Spotify's recommendations for that
    last song, and on another device, Soloist included, it plays that song's
    radio once the device stands stopped at the start of it, unless something
    else was started meanwhile. With Autoplay off, playback stops there.

14. **Recent is the player's history when it gives one.** On Soloist, the
    queue panel's **Recent** tab lists what Soloist itself played before
    the current song, most recent first, skipped songs included: the songs
    Previous steps back through, as in Spotify's own panel. Elsewhere it
    merges Spotify's recently played list with this computer's history.
