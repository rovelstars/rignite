# Image optimization
Initially we were converting svg images to png using rsvg-convert to 512x512 size. But the resulting pngs were quite large (100-200KB each).
We now convert the PNGs to QOI format, and use a QOI encoder and decoder library to reduce file size from initial 7.5MB EFI image to just 812KB.
**UPDATE:** I found out that `logo.svg` had a lot of unnecessary details that were not visible at small sizes. I simplified the SVG and re-exported to PNG, then converted to QOI again. This reduced the QOI file from 160KB to just 20KB!

# Switching from ab_glyph to fontdue for font rendering
The switch to `fontdue` offers faster performance and lower memory usage compared to `ab_glyph`, resulting in a more efficient application.
However this initially increased binary size for about 30KB due to hashmap algorithms and other stuff.
However we were able to reduce binary size by enabling these:
```
opt-level = "z"
codegen-units = 1
strip = true
```
this reduced binary size back to around 844KB.

# Replacing `log` crate with built-in simple logger macros
The `log` crate, while versatile, adds significant overhead to the binary size due to its extensive features and abstractions. We reduced another 20KB by replacing it with simple custom logging macros that provide only the necessary functionality for our application. Now our EFI binary size is around 820KB.

# Font size reduction
We downloaded stock Jetbrains Mono font (regular TTF).

We run this to only keep basic English, numbers, and common symbols
```
pyftsubset JBMR.ttf --unicodes="U+0020-007E" --output-file=JBMR_subset.ttf
```

Result:
```
❯ du -sh JBMR_subset.ttf
68K	JBMR_subset.ttf
❯ du -sh JBMR.ttf
268K	JBMR.ttf
```

Our current EFI binary size is around **616KB** with the subsetted font!
