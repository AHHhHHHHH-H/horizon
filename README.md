# Design review screenshots (not for merging)

Screenshots for the design-system stack (#166 tokens, #167 mark and icons, #168 launcher,
the game menus and the website PRs). This branch only holds images for the PR bodies; it is
never merged. Before = main at d8ad1dad (2026-10-14), after = the stack.

- `launcher/`: every page, before/after (dark) and light/dark pairs.
- `states/`: launcher states: opening, picker, empty, unprepared, error, crash.
- `menus/`: every Horizon menu screen, before/after.
- `classic/`: vanilla's classic screens from both builds (unchanged).
- `website/`: folds and full pages at 1440 and 390, light and dark, before/after.
- `icon/`: the three candidates at 16–256 on light and dark, the pick everywhere, the social
  images and avatars.

## Round 2 (after Ludovic's answers)

- `round2/menus/`: every Horizon menu screen before/after, and `*-round1-round2.png` for what
  changed: 0.90 veil on pages and in a world, 0.80 on home with ink_soft text, the whole
  wordmark.
- `round2/launcher/`: every page before/after and light/dark, with #164's account rows restyled
  and the Applications-menu switch (Linux).
- `round2/packaging/`: the Windows exes' icon resource (from the CI artifacts), the Linux app
  menu entry (fuzzel), the .dmg window (a mock from the dmg's real layout and background; a
  Finder window can't be captured over SSH) and its background.
