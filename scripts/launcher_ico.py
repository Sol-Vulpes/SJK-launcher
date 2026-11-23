"""The launchers' Windows icon files, made from their PNG icons so Explorer, the
taskbar's pinned entry and shortcuts show the same badge as the window and tray.

    python scripts/launcher_ico.py

Needs Pillow. Writes assets/launcher-icon.ico (SJK Launcher) and
assets/jof/launcher-icon.ico (JoF Launcher); build.rs embeds the brand's one into the
exe. Run again after changing a launcher-icon.png.
"""

from pathlib import Path

from PIL import Image

SIZES = [(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)]
ROOT = Path(__file__).resolve().parent.parent

for folder in (ROOT / "assets", ROOT / "assets" / "jof"):
    icon = Image.open(folder / "launcher-icon.png").convert("RGBA")
    icon.save(folder / "launcher-icon.ico", sizes=SIZES)
    print(folder / "launcher-icon.ico")
