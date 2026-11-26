#!/usr/bin/env python3
"""Собирает `crates/dj-controller-s4mk3/data/farewell-icon.565` — иконку приложения в формате экранов панели.

Экраны принимают RGB565, а PNG приносить в драйвер незачем: картинка одна и меняется раз в год, поэтому она
переводится в 16-битные слова один раз здесь, а код встраивает готовый файл через `include_bytes!` — без
декодера PNG в дереве зависимостей и без таблицы на сто тридцать килобайт в исходнике.

Альфа-канал складывается с фоном экрана (`screen::BACKGROUND`, 0x0841) прямо здесь: у RGB565 альфы нет, а
прозрачные углы иконки должны стать тёмными углами, а не чёрными квадратами.

Запуск:
  nix shell nixpkgs#imagemagick -c magick "assets/app-icon/Hauler DJ.128.png" -filter Lanczos \
      -resize 224x224 -depth 8 rgba:- > /tmp/icon.rgba
  nix shell nixpkgs#python3 -c python3 scripts/make-screen-icon.py /tmp/icon.rgba \\
      crates/dj-controller-s4mk3/data/farewell-icon.565
"""
import sys
from pathlib import Path

# The side of the dump is taken from the file itself: the icon is resized by ImageMagick before this script runs (224 is
# what the panel wants), and a hard-coded side is one more number to forget. `src/icon.rs` carries the same number, and
# `tests/farewell.rs` fails if the two drift apart.
# `screen::BACKGROUND` — 0x0841 — taken apart into its own five, six and five bit fields. The blend happens in those
# fields and not in eight-bit colour: a round trip through 888 would land a transparent corner one step away from the
# background it is meant to be.
BACKGROUND = 0x0841


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    raw = Path(sys.argv[1]).read_bytes()
    size = int((len(raw) // 4) ** 0.5)
    if size * size * 4 != len(raw):
        raise SystemExit(f"{sys.argv[1]}: {len(raw)} bytes, which is not a square RGBA picture")

    out = bytearray()
    background = (
        (BACKGROUND >> 11) & 0x1F,
        (BACKGROUND >> 5) & 0x3F,
        BACKGROUND & 0x1F,
    )
    for index in range(0, len(raw), 4):
        red, green, blue, alpha = raw[index:index + 4]
        fields = (red >> 3, green >> 2, blue >> 3)
        word = 0
        for value, there, bits in zip(fields, background, (5, 6, 5)):
            blended = (value * alpha + there * (255 - alpha) + 127) // 255
            word = (word << bits) | (blended & ((1 << bits) - 1))
        out += word.to_bytes(2, "little")
    target = Path(sys.argv[2])
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(out)
    print(f"{target}: {len(out)} bytes, {size} by {size} pixels of RGB565 — SIZE in src/icon.rs must be {size}")


main()
