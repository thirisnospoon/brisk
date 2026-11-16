"""Draws the Brisk icon (assets/brisk.png, assets/brisk.ico). Needs Pillow: `python assets/make_icon.py`."""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont, ImageFilter

HERE = Path(__file__).parent
S = 1024


def gradient(size, top, bottom):
    g = Image.new("RGB", (1, size))
    for y in range(size):
        t = y / (size - 1)
        g.putpixel((0, y), tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)))
    return g.resize((size, size))


def draw():
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    # Rounded square with a diagonal indigo-to-blue gradient.
    bg = gradient(S, (88, 70, 230), (20, 150, 235)).rotate(-35, resample=Image.BICUBIC, expand=False)
    bg = bg.resize((int(S * 1.6), int(S * 1.6))).crop((int(S * .3), int(S * .3), int(S * 1.3), int(S * 1.3)))
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle((40, 40, S - 40, S - 40), radius=210, fill=255)
    img.paste(bg, (0, 0), mask)

    d = ImageDraw.Draw(img)
    # Speed lines to the left of the letter.
    for y, x0, w in ((380, 150, 170), (512, 110, 210), (644, 150, 170)):
        d.rounded_rectangle((x0, y - 26, x0 + w, y + 26), radius=26, fill=(255, 255, 255, 150))

    # Bold "B", slanted for motion.
    font = None
    for name in ("segoeuib.ttf", "arialbd.ttf", "DejaVuSans-Bold.ttf"):
        try:
            font = ImageFont.truetype(name, 640)
            break
        except OSError:
            pass
    letter = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ld = ImageDraw.Draw(letter)
    box = ld.textbbox((0, 0), "B", font=font)
    w, h = box[2] - box[0], box[3] - box[1]
    ld.text(((S - w) / 2 - box[0] + 70, (S - h) / 2 - box[1]), "B", font=font, fill=(255, 255, 255, 255))
    letter = letter.transform((S, S), Image.AFFINE, (1, 0.18, -95, 0, 1, 0), resample=Image.BICUBIC)
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    shadow.putalpha(letter.getchannel("A").point(lambda a: a * 0.35))
    shadow = shadow.filter(ImageFilter.GaussianBlur(14))
    img.alpha_composite(shadow, (10, 16))
    img.alpha_composite(letter)
    return img


def main():
    img = draw()
    img.resize((256, 256), Image.LANCZOS).save(HERE / "brisk.png")
    sizes = [(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)]
    img.save(HERE / "brisk.ico", sizes=sizes)


if __name__ == "__main__":
    main()
