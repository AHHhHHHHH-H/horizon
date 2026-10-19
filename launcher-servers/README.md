# Launcher Servers page: screenshots and button names (for site-docs-1)

Launcher branch `feat/launcher-servers-page` (PR to come after #1009 and #1017). The screens were captured from the
real launcher (debug build) on the ThinkPad (Linux, Xvfb + Mesa lavapipe, driven with xdotool) at 1180x860. The
Mac mini's session was locked, so `screencapture` saw only the lock screen. The launcher ran in its own network
namespace, so the addresses are made up: `10.0.2.100` on the LAN. Tailscale was a stand-in CLI and API (MagicDNS
`ludos-pc.tail1234.ts.net`, IP `100.64.12.34`, emails `@example.com`). `40-mac-client-joined.png` is a real Horizon
client joined to a server the launcher's commands set up on the Mac mini.

## Where things are

- Top navigation: **Servers** (between Modpacks and Patch notes). The Play page has a **Host a server** button that
  leads there.
- Servers page: **Create a server**, **Add a server folder…**; one row per server with **Open**.
- Create dialog (`02`–`05`): Name; World: **New world** / **Copy one of my worlds** (with a seed field for a new
  world); Game mode: **Survival** / **Creative** / **Adventure**; Difficulty: **Peaceful** / **Easy** / **Normal** /
  **Hard**; Most players at once: **–** / **+**; Who can join: **Friends only** / **Anyone with the address**, then
  the players' names (the first one becomes an operator); **Check Minecraft accounts** checkbox; **I agree to the
  Minecraft EULA** checkbox with a **Read it** link; **Cancel**, **Create**. Escape closes it. With the defaults,
  tick the EULA and press Create.
- A server (`06`, `07`): **← All servers**; state line (Stopped, Starting, Running, Running · paused while nobody
  plays, Running · restarted after a crash, Restarting, Stopping, Updating, with the uptime; after a crash, "The
  game crashed N times and was restarted by itself (last: …)", `41`); **Start** / **Stop**, **Restart**, **Open the web
  panel**, **Open the folder**; tiles Players online, TPS (20 is full speed), MSPT (tick time), RAM. Tabs
  **Overview**, **Players**, **Settings**, **Backups**.
- Overview: Address on this network with **Copy** (`:port` added when it isn't 25565) and the hint for friends
  on the same Wi-Fi; Friends over the internet (Tailscale); Console with a command line (Enter sends).
- Friends over the internet (`10`–`19`):
  - Not installed: **Install Tailscale**, **Check again**.
  - Not running: **Open Tailscale**, **Check again**.
  - Signed out: **Sign in to Tailscale** (Tailscale's own sign-in; on Linux the page shows
    `sudo tailscale up --operator=$USER`), **Check again**.
  - Running: the MagicDNS name (and IP) with **Copy**.
  - Invite a friend: **Open the admin console** or **Use an access token…** (field `tskey-api-…`, **Save**,
    **Make a token** link).
  - With a token: Their email (optional), **Make an invitation**, the invite link with **Copy**; a list with
    **Withdraw** (not accepted yet) or **Stop sharing** (accepted).
  - Access: **Limit them to Minecraft** → **Change the access controls** / **Cancel**; **Remove the limit**.
  - Your own access controls: a rule to paste, **Copy**, **Open access controls**.
  - **Forget the access token**. Horizon never turns on Funnel.
- Players (`20`–`22`): Online now with heads, **Make operator** / **Remove operator**, **Kick**, **Ban**. Who may
  join: **Friends only** switch, A player's name + **Let in**, the list with **Make operator** / **Remove
  operator** and **Remove**.
- Settings (`23`–`26`): Message in the server list (+ **Save**), Game mode, Difficulty, Players can hurt each other
  (PvP), Most players at once, View distance, Simulation distance, Port (+ **Save**), Check Minecraft accounts,
  Run in the background and at login. Each says whether it applies at once or after a restart. A banner then
  says "Saved. Restart to apply: …" with **Restart now**. Refused values show in red under the field.
- Errors (`27`, `28`): plain words, with a fix button where there is one (**Use port 25566**, **Prepare the game
  files**) and **Dismiss**.
- Backups (`29`–`36`): **Back up now**, Back up automatically switch (every 6h, keeping the last 12; hand-made
  backups are never deleted), the list with **Restore** → **Replace the world** / **Cancel**. Updates: "This
  server's Horizon" with **Check for updates** (**Checking…**) or **Update now** ("Update available: Horizon … The
  world is backed up first; it goes back by itself if the new version doesn't start"), and Older versions kept
  (**–** / **+**).
- Keyboard: Tab / Shift-Tab move through every control (accent ring with a gap), Enter or Space presses,
  Escape closes the Create dialog. Light and dark follow the system (`38`, `39` are dark).
