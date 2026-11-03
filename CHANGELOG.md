# Changelog

## 0.7.7 beta (2026-11-03)

Security review of 0.7.6.

- Security: the local plugin server now only answers pages it served itself (its own address and port). Pages from other local servers on this PC, such as a dev server, are turned away.
- Security: the intro video and screen-saver GIF must be a file on this PC. Network paths (\\server\share) are refused, so picking one can't send your Windows sign-in to another computer, and the intro video must really be an MP4 or WebM file.

## 0.7.6 beta (2026-11-01)

Fixes from the design panel's review of 0.7.4.

- New Settings page: theme, Start with Windows, the window intro and the hold time. The Dock page is now two even cards, Screen and Light ring, with the screen saver on one row.
- Key editor header: "← All keys (Esc)", the key's name, then Change action, Press now and Remove.
- Right-click menu works from the keyboard (Shift+F10 or the menu key, arrows, Home/End, Esc) and on empty keys. New items: Press now, Copy, Paste, Duplicate to next free key and Move to page. Delete removes the selected key, Ctrl+C / Ctrl+V copy and paste it.
- Keys with a hold action show a gilded corner and "hold: …" in the key list. "When held" sits under the action's settings. Hold time can be set from 0.3 to 1 s.
- Profile row: Profile, New and a ⋯ menu (Rename, Apps, Import, Delete). Deleting a page is an icon on its tab. Bigger key grid with the key list under it.
- Better contrast and focus outlines: switch tracks, menus, selects, text areas and search; dark keys get an edge; ring buttons report their state.
- On 5 x 3 docks the big clock, temple and starfield are drawn key by key (one digit per key, the temple on the middle keys, twinkling stars on every key).
- The window intro now plays after an update by default (or every time, your video, or nothing) and is skipped when Windows asks for fewer animations.

## 0.7.5 beta (2026-11-01)

- Security: Agora's local plugin server now only answers Agora's own pages and programs on this PC. A website open in your browser can't reach it, by its Origin or by a rebound domain name.
- Security: Agora's own keys can't be changed over the plugin connection, and a running plugin can't be taken over by a second connection using its id.
- Screen saver GIFs larger than 2048 x 2048 pixels are refused.
- Dock page: cards stack in two columns without gaps, and switches sit beside their text.
- Numbers in headings and on keys read as numbers ("10", not "IO").

## 0.7.4 beta (2026-11-01)

- Hold for a second action: any key can run its own list of actions when held for half a second; a quick press still runs the key.
- New keys: Hotkey switch, Random action, Clock, Timer, Stopwatch, Counter, Audio output (next output or a chosen one), App volume, Web request, Run command (PowerShell, can show its output), Snippets (a picker beside the mouse) and Window (next screen, snap, maximise, keep on top).
- Open keys take the app's own icon when you pick a program.
- Screen saver on the dock after it sits unused: Matrix rain, the Agora temple, a starfield, a big clock or your own GIF. Any press stops it.
- Intro: the Agora temple builds itself on the dock when it connects, and in the window when Agora opens. You can play your own video instead, or turn it off.

## 0.7.3 beta (2026-11-01)

- New app icon: a temple of keys.

## 0.7.2 beta (2026-11-01)

- Agora's built-in keys give a quick bounce on the dock when you press them.

## 0.7.1 beta (2026-11-01)

- Right-click a key to edit it or remove it (Remove key can be undone with Undo or Ctrl+Z).

## 0.7.0 beta (2026-11-01)

Agora's own versions of the VSD Craft built-ins it was missing, written for Agora:

- Monitor keys: CPU, memory, disk (how full or how busy), GPU and GPU memory, updated every 2 seconds, with a bar. IP address shows your public address (looked up through OpenDNS, press to refresh) or your local one.
- Quick control: sleep, lock, show desktop, turn the screens off, mute the microphone, switch screens.
- Quick tool: Calculator, emoji panel, Task Manager, search, notifications, Settings, Control Panel, File Explorer, screenshot, clipboard history, Run and new email.
- Close window, Mouse (click, scroll, move), Browser (back, forward, refresh, stop, home and tabs).
- Password: types a password saved in Windows Credential Manager; it never goes in Agora's files.
- Soundboard: play an MP3, WAV or other sound at a set volume, or stop all sounds.
- Network message: send a UDP message, as text or hex bytes.
- Dock sleep: turns the dock's screen and ring off until the next press.

## 0.6.2 beta (2026-11-01)

- VSD Craft import brings over more keys: Open Apps (opened through the Start menu, with the app's picture), Page Indicator, and round buttons set to the previous or next scene.
- New Page indicator key: shows "page n / m" and goes to the next page when pressed.
- Switch profile can now go to the next or previous profile.
- Fixed: imported Scene Shift keys that point at another imported profile now switch to it.

## 0.6.1 beta (2026-10-31)
- Light or dark: choose Like Windows, Light or Dark on the Dock page, or flip it with the sun/moon button at the foot of the side bar. Agora remembers it, and the window's title bar follows.

## 0.6.0 beta (2026-10-31)
The first public release. Tested on the Stream Dock M18 only.

- Windows installer: installs for you only (no administrator rights), Start menu entry, uninstaller that keeps your keys and settings.
- Docks: Stream Dock M18 (tested); M18 V3, 293V3, 293S, N3, N4 / N4 Pro and XL (untested, from public notes).
- Runs Stream Dock plugins unchanged (Node.js, HTML and .exe), with their settings panels. Tested with Winged Keys and Shannon Keys.
- Profiles, pages and folders; profiles that follow the app in front.
- Agora's own actions: open, website, hotkey, type text, media keys, brightness, light ring, page and profile navigation, folders.
- Multi-actions, Toggle, and Knob actions for docks with knobs.
- The M18's round buttons hold actions.
- Light ring in one colour or painted light by light, with presets.
- Start with Windows, in the tray.
- Import of M18 profiles from VSD Craft (read only).
- Fast page changes on the dock and in the window: one question to the host per page change, previews drawn together.
- Undo (or Ctrl+Z) for a few seconds after removing a key, page, profile or plugin, instead of "Sure? Delete".
- One status area at the foot of every page for errors, "Saved" and Undo, read out by screen readers.
- Easier to see and use: stronger edges on fields and buttons, bigger buttons and text, keys named for screen readers, a Rename button on the page tab and an Open button for folders.
- Home shows the dock, profile and page, and offers to import your VSD Craft profiles when it finds them.
- The Inter and GFS Didot fonts now ship with Agora.
- Plugins: VSD Craft's Node 20 is preferred over another Node on PATH; "Information" actions (such as OBS monitors) go on keys; plugins can switch profiles and show alerts on knobs; plugins packed one folder too deep are found.

Support Agora on [Ko-fi](https://ko-fi.com/overturesys).
