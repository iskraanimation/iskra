"""sheet.py <boards> <out prefix> <models: dir:label,...> <panel:box ...>: rows of panels, a column per model,
each crop showing the found joints joined by the support's lines, joint names small."""
import sys, json, cv2, numpy as np
B, pre, models = sys.argv[1], sys.argv[2], [m.split(':') for m in sys.argv[3].split(',')]
items = [(p.split(':')[0], list(map(int, p.split(':')[1].split(',')))) for p in sys.argv[4:]]
H = 330
rows = []
for n, (p, (x0, y0, x1, y1)) in enumerate(items):
    img = cv2.imread(f'{B}/Layer {p}.jpg'); cells = []
    for d, label in models:
        r = json.load(open(f'{d}/Layer_{p}_{n}.json')); sk = json.load(open(r.get('skel', 'real/skel.json'))) if False else None
        c = img.copy()
        pts = [tuple(map(int, q)) for q in r['points']]
        for i, q in enumerate(pts):
            cv2.circle(c, q, 9, (0, 0, 230), -1)
            cv2.putText(c, r['names'][i].replace('_', ''), (q[0] + 10, q[1] + 5), 0, 0.9, (0, 0, 160), 2)
        c = c[y0:y1, x0:x1]; c = cv2.resize(c, (int(c.shape[1] * H / c.shape[0]), H))
        cv2.putText(c, f'#{p} {label}', (5, 25), 0, 0.8, (0, 0, 0), 2); cells.append(c)
    rows.append(np.hstack([cv2.copyMakeBorder(c, 2, 2, 2, 2, cv2.BORDER_CONSTANT, value=(0, 0, 0)) for c in cells]))
W = max(r.shape[1] for r in rows)
rows = [cv2.copyMakeBorder(r, 0, 0, 0, W - r.shape[1], cv2.BORDER_CONSTANT, value=(255, 255, 255)) for r in rows]
for k in range(0, len(rows), 3):
    cv2.imwrite(f'{pre}_{k // 3}.png', np.vstack(rows[k:k + 3]))
