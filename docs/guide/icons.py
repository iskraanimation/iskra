"""Puts the app's icons that the quick guide uses into it.

    python3 docs/guide/icons.py

The guide shows an icon as `<svg class="ic"><use href="#i-brush"/></svg>`, in the colour of
the text round it, as the app tints its icons. This writes each icon used, from
crates/ui/assets/icons, as a symbol into the block between the `icons:` markers at the top of
quick-guide.html. Run it after using an icon that is not there yet.
"""

import pathlib
import re

here = pathlib.Path(__file__).resolve().parent
icons = here.parent.parent / "crates" / "ui" / "assets" / "icons"
page = here / "quick-guide.html"

html = page.read_text()
used = sorted(set(re.findall(r'<use href="#i-([a-z0-9-]+)"', html)))
symbols = []
for name in used:
    svg = (icons / f"{name}.svg").read_text()
    attributes = re.search(r"<svg([^>]*)>", svg).group(1)
    inside = svg[svg.index(">", svg.index("<svg")) + 1 : svg.rindex("</svg>")]
    # What the symbol needs of the icon's own <svg>: its box and how it is stroked.
    wanted = r'\s(viewBox|fill|stroke|stroke-width|stroke-linecap|stroke-linejoin)="([^"]*)"'
    keep = " ".join(f'{key}="{value}"' for key, value in re.findall(wanted, attributes))
    symbols.append(f'<symbol id="i-{name}" {keep}>{" ".join(inside.split())}</symbol>')
block = '<!-- icons: made by icons.py -->\n<svg style="display:none">\n' + "\n".join(symbols) + "\n</svg>\n<!-- icons: end -->"
html, count = re.subn(r"<!-- icons: made by icons\.py -->.*?<!-- icons: end -->", lambda _: block, html, flags=re.S)
assert count == 1, "the page has the icons block once"
page.write_text(html)
print(f"{len(used)} icons: {', '.join(used)}")
