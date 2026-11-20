#!/usr/bin/env python3
"""Собирает app.ico из PNG-иконок проекта: один рисунок, четыре размера.

ICO — это каталог картинок: заголовок из шести байт, по записи на размер и сами данные. Для 64 px и больше
Windows принимает PNG внутри ICO как есть, поэтому пересжимать ничего не нужно — берётся тот же файл, что дал
владелец.

Запуск: python3 scripts/make-icon.py assets/app-icon/*.png assets/app-icon/app.ico
"""
import struct
import sys
from pathlib import Path


def png_size(path: Path) -> tuple[int, int]:
    header = path.read_bytes()[:33]
    if header[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"{path}: это не PNG")
    width, height = struct.unpack(">II", header[16:24])
    return width, height


def build(sources: list[Path], target: Path) -> None:
    images = []
    for source in sources:
        width, height = png_size(source)
        if width != height:
            raise SystemExit(f"{source}: иконка должна быть квадратной, а она {width}x{height}")
        images.append((width, height, source.read_bytes()))
    images.sort(key=lambda entry: entry[0])

    out = bytearray()
    out += struct.pack("<HHH", 0, 1, len(images))  # reserved, type = icon, count
    offset = 6 + 16 * len(images)
    for width, height, payload in images:
        # 256 записывается как 0 — единственное место, где ICO не умеет «просто число».
        out += struct.pack(
            "<BBBBHHII",
            0 if width >= 256 else width,
            0 if height >= 256 else height,
            0,  # палитра
            0,  # reserved
            1,  # planes
            32,  # bits per pixel
            len(payload),
            offset,
        )
        offset += len(payload)
    for _, _, payload in images:
        out += payload
    target.write_bytes(bytes(out))


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    sources = [Path(name) for name in sys.argv[1:-1]]
    build(sources, Path(sys.argv[-1]))
    sizes = ", ".join(str(png_size(source)[0]) for source in sources)
    print(f"icon: {sys.argv[-1]} ({sizes})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
