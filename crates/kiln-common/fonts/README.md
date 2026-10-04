# Bundled fonts

Pretendard and JetBrains Mono retain their existing licenses in this directory.

## Noto Sans CJK Regular

`NotoSansCJK-Regular.ttc` is an **unmodified** upstream Noto Sans CJK 2.004
OpenType Collection, licensed under the [SIL Open Font License 1.1](NotoSansCJK-OFL.txt).
Copyright 2014–2021 Adobe (http://www.adobe.com/), as recorded in the font metadata.

- [Project and available formats](https://github.com/notofonts/noto-cjk/blob/main/Sans/README.md)
- [Pinned original font](https://github.com/notofonts/noto-cjk/blob/Sans2.004/Sans/OTC/NotoSansCJK-Regular.ttc)
- [Upstream license](https://github.com/notofonts/noto-cjk/blob/main/Sans/LICENSE)
- Size: 19,484,784 bytes (18.58 MiB).
- SHA-256: `b76b0433203017ca80401b2ee0dd69350349871c4b19d504c34dbdd80541690a`.

The collection shares outlines across regional faces, avoiding separate copies of
large Japanese and Chinese font files. Kiln selects face 0 (Japanese), 1 (Korean,
also the English UI default), or 2 (Simplified Chinese) through `FontData.index`.
The complete upstream glyph coverage is retained; it is not subsetted to the UI's
current translations. Only the Regular weight is included.

Every UI family includes this bundled fallback before optional system fonts. The
bundled Pretendard weights supply Latin, Hangul and kana but contain no Han
characters, so Japanese and Chinese Han use the selected regional Noto face.
JetBrains Mono remains the primary font for monospace text. No installed system
CJK font or network request is needed at runtime or in headless GUI tests.

To replace or audit the binary, download the pinned file and verify its digest:

```sh
curl -fL https://raw.githubusercontent.com/notofonts/noto-cjk/Sans2.004/Sans/OTC/NotoSansCJK-Regular.ttc -o NotoSansCJK-Regular.ttc
shasum -a 256 NotoSansCJK-Regular.ttc
```
