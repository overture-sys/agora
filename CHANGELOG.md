# Changelog

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
