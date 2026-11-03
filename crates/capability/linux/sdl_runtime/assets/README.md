# RUDROID startup mascot

Jenny92 requested a new ordinary purple-cat drawing instead of reusing the
APP Manager icon. `mascot-source.png` is the new built-in image_gen output:
a seated purple cat with round glasses, empty paws and a curled tail, with
real transparent alpha. There are no tools, gears, icon tile or baked text.
The APP Manager icon was used only as a character/style reference, not as
the final asset. Its source remains unchanged in the launcher repository at
`ports/appmanager/trimui-app/icon.png` (SHA256
`c402bbef2499d6d73f6cadf6a0cd6030fe0668bc5f33bd2b26d9245b0d72febd`).

Generation used the built-in image_gen tool with `transparent_background=true`;
the exact final prompt is in [mascot-prompt.txt](mascot-prompt.txt).

The runtime PNG is an alpha-preserving 300x300 export of the generated master:

```sh
sips --resampleHeightWidth 300 300 mascot-source.png --out mascot.png
```

`mascot.rgba` losslessly decodes `mascot.png` into top-to-bottom row-major RGBA8
with straight alpha (360,000 bytes). The embedded asset needs no decoder,
file lookup or new dependency on a handheld. The one-shot shader composites
its alpha over the dark background; the temporary texture and geometry are
released before guest activation, with no animation, wait or gameplay work.
Embedded `.rgba` bytes participate in the kit fingerprint. RUDROID and the
Unity Loader subtitle are drawn separately by `startup_brand.rs`.

Generated master PNG SHA256: `1497dd928cc9593f807cb9c1978704ee59ba14e856ec6557610712887f0bafa7`

Runtime PNG SHA256: `d6df09f76ab873f84173613dda8e47d9999ad8c2188660516de7a8b48c3efd82`

Decoded RGBA SHA256: `fdbb78cbdf613946f258df139f6d30d956869a1e93c398b226c29ea9e9c1e0eb`
