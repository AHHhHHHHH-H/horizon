# Effect Lab

An original API demonstration, **not** a port of any of the requested mods.
Requires a Horizon build serving API level 18.

Build with `cargo horizon-mod build` in this folder. The resulting `.hmod`
contains code, the original spark PNG and settings. Put it in a scratch game
home's `horizon-mods` folder and enable it in Options → Mods. Then enable
Effects in its Settings, or use `/effect_lab on` (`off` clears the controls).
The toggle is saved in the mod's storage; default is off.

This shows a blue local light, original floating spark sprites, gentle bloom,
an original blue patterned stone overlay, and opt-in `_e`/`_emissive` overlays
if the active resource pack has them.
Bloom pauses while a shader pack owns post-processing. No assets are copied
from Minecraft or other mods. Reload content assets/restart after changing PNGs.
