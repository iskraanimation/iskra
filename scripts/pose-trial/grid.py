"""grid.py <picture> <x0,y0,x1,y1> <out> [points.json]: the crop with a labelled grid (every
20 px, every 100 px in red), scaled by $K (default 1); points of the json marked in green."""
import json, os, sys

import cv2

K = float(os.environ.get('K', '1'))
img = cv2.imread(sys.argv[1])
x0, y0, x1, y1 = map(int, sys.argv[2].split(','))
c = cv2.resize(img[y0:y1, x0:x1], None, fx=K, fy=K, interpolation=cv2.INTER_AREA)
P = lambda x, y: (int(round((x - x0) * K)), int(round((y - y0) * K)))
for x in range((x0 // 20 + 1) * 20, x1, 20):
    red = x % 100 == 0
    cv2.line(c, P(x, y0), P(x, y1), (0, 0, 255) if red else (215, 215, 255), 1)
    if red:
        cv2.putText(c, str(x), (P(x, y0)[0] + 2, 14), 0, 0.45, (0, 0, 255), 1)
for y in range((y0 // 20 + 1) * 20, y1, 20):
    red = y % 100 == 0
    cv2.line(c, P(x0, y), P(x1, y), (0, 0, 255) if red else (215, 215, 255), 1)
    if red:
        cv2.putText(c, str(y), (2, P(x0, y)[1] - 2), 0, 0.45, (0, 0, 255), 1)
if len(sys.argv) > 4:
    d = json.load(open(sys.argv[4]))
    for n, (x, y) in zip(d['names'], d['points']):
        cv2.circle(c, P(x, y), 5, (0, 160, 0), -1)
        cv2.putText(c, n, (P(x, y)[0] + 6, P(x, y)[1]), 0, 0.4, (0, 110, 0), 1)
cv2.imwrite(sys.argv[3], c)
