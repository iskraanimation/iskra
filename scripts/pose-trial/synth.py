"""A cartoon character not of human scale (big head, stubby limbs, a tail), posed by FK.

Writes, into the out dir:
  support.png + support.json   the "rig" at rest, coloured and outlined, joints known
  <pose>_render.png            the rig in a new pose, as Iskra would draw it
  <pose>_sketch.png            the same pose as a loose pencil sketch: wobbly, doubled
                               outlines, proportions a little off
  truth.json                   the true joints of every query
"""
import json, math, os, random, sys

import cv2
import numpy as np

NAMES = ['Hips', 'Chest', 'Neck', 'Head_Top', 'Eye_L', 'Eye_R', 'Mouth',
         'Shoulder_L', 'Elbow_L', 'Wrist_L', 'Shoulder_R', 'Elbow_R', 'Wrist_R',
         'Hip_L', 'Knee_L', 'Ankle_L', 'Hip_R', 'Knee_R', 'Ankle_R', 'Tail1', 'Tail2', 'TailEnd']
SKEL = [('Hips', 'Chest'), ('Chest', 'Neck'), ('Neck', 'Head_Top'), ('Neck', 'Eye_L'), ('Neck', 'Eye_R'),
        ('Neck', 'Mouth'), ('Chest', 'Shoulder_L'), ('Shoulder_L', 'Elbow_L'), ('Elbow_L', 'Wrist_L'),
        ('Chest', 'Shoulder_R'), ('Shoulder_R', 'Elbow_R'), ('Elbow_R', 'Wrist_R'), ('Hips', 'Hip_L'),
        ('Hip_L', 'Knee_L'), ('Knee_L', 'Ankle_L'), ('Hips', 'Hip_R'), ('Hip_R', 'Knee_R'), ('Knee_R', 'Ankle_R'),
        ('Hips', 'Tail1'), ('Tail1', 'Tail2'), ('Tail2', 'TailEnd')]

# Pose: angles in degrees (0 = right, 90 = down on screen), and the face's slide in the head.
REST = dict(spine=-90, head=-90, arm_l=[200, 160], arm_r=[-20, 20], leg_l=[115, 100], leg_r=[65, 80],
            tail=[10, -20, -50], face=(0, 0), root=(0, 0))
POSES = {
    'wave': dict(spine=-80, head=-70, arm_l=[200, 250], arm_r=[-30, -80], leg_l=[110, 95], leg_r=[70, 85],
                 tail=[0, -30, -70], face=(18, 4), root=(30, 0)),
    'run': dict(spine=-65, head=-60, arm_l=[60, -10], arm_r=[180, 130], leg_l=[150, 110], leg_r=[30, 110],
                tail=[170, 190, 220], face=(25, 0), root=(-20, -15)),
    'crouch': dict(spine=-110, head=-120, arm_l=[100, 40], arm_r=[80, 20], leg_l=[160, 60], leg_r=[20, 120],
                   tail=[-10, 30, 80], face=(-20, 10), root=(0, 40)),
    'reach': dict(spine=-95, head=-100, arm_l=[-120, -100], arm_r=[-60, -80], leg_l=[95, 95], leg_r=[85, 85],
                  tail=[10, 40, 10], face=(-6, -14), root=(0, 0)),
}
EXTRA = [
    dict(spine=-90, head=-95, arm_l=[230, 200], arm_r=[-50, -20], leg_l=[120, 90], leg_r=[60, 90], tail=[0, -40, -80], face=(10, 0), root=(0, 0)),
    dict(spine=-85, head=-80, arm_l=[170, 100], arm_r=[10, 80], leg_l=[130, 80], leg_r=[70, 110], tail=[30, 10, -20], face=(-12, 4), root=(0, 0)),
    dict(spine=-100, head=-100, arm_l=[250, 280], arm_r=[40, 100], leg_l=[100, 100], leg_r=[50, 70], tail=[-20, -60, -90], face=(0, -10), root=(0, 0)),
    dict(spine=-75, head=-85, arm_l=[140, 190], arm_r=[-10, -60], leg_l=[140, 100], leg_r=[90, 70], tail=[40, 60, 30], face=(14, 8), root=(0, 0)),
]
# Lengths: head radius big, limbs stubby.
L = dict(spine=70, neck=14, head_r=62, upper=38, lower=34, thigh=34, shin=30, tail=[34, 30, 26], sh=34, hp=18)


def pt(o, ang, ln):
    return (o[0] + ln * math.cos(math.radians(ang)), o[1] + ln * math.sin(math.radians(ang)))


def joints(p, scale=1.0, jitter=None):
    r = random.Random(jitter) if jitter is not None else None
    k = (lambda: 1 + r.uniform(-0.15, 0.15)) if r else (lambda: 1.0)
    J = {}
    hips = (300 + p['root'][0], 330 + p['root'][1])
    J['Hips'] = hips
    J['Chest'] = pt(hips, p['spine'], L['spine'] * k())
    J['Neck'] = pt(J['Chest'], p['spine'], L['neck'] * k())
    hc = pt(J['Neck'], p['head'], L['head_r'] * k())
    J['Head_Top'] = pt(J['Neck'], p['head'], 2 * L['head_r'])
    fx, fy = p['face']
    perp = p['head'] + 90
    side = lambda d: pt(hc, perp, d)
    c = (hc[0] + fx, hc[1] + fy)
    J['Eye_L'] = pt(pt(c, perp, -20), p['head'], 8)
    J['Eye_R'] = pt(pt(c, perp, 20), p['head'], 8)
    J['Mouth'] = pt(c, p['head'], -24)
    J['Shoulder_L'] = pt(J['Chest'], p['spine'] - 90, L['sh'] * 0.5)
    J['Shoulder_R'] = pt(J['Chest'], p['spine'] + 90, L['sh'] * 0.5)
    J['Shoulder_L'], J['Shoulder_R'] = J['Shoulder_R'], J['Shoulder_L']
    for s in 'LR':
        a = p['arm_' + s.lower()]
        J['Elbow_' + s] = pt(J['Shoulder_' + s], a[0], L['upper'] * k())
        J['Wrist_' + s] = pt(J['Elbow_' + s], a[1], L['lower'] * k())
    J['Hip_L'] = pt(hips, p['spine'] + 90, L['hp'])
    J['Hip_R'] = pt(hips, p['spine'] - 90, L['hp'])
    J['Hip_L'], J['Hip_R'] = J['Hip_R'], J['Hip_L']
    for s in 'LR':
        a = p['leg_' + s.lower()]
        J['Knee_' + s] = pt(J['Hip_' + s], a[0], L['thigh'] * k())
        J['Ankle_' + s] = pt(J['Knee_' + s], a[1], L['shin'] * k())
    t = hips
    for n, a, ln in zip(['Tail1', 'Tail2', 'TailEnd'], p['tail'], L['tail']):
        t = pt(t, a, ln * k())
        J[n] = t
    return J, hc


def ip(p):
    return (int(round(p[0])), int(round(p[1])))


def render(J, hc, p):
    img = np.full((520, 600, 3), 255, np.uint8)
    body, limb, head = (90, 160, 230), (60, 120, 200), (120, 190, 250)

    def capsule(a, b, w, col):
        cv2.line(img, ip(a), ip(b), (30, 30, 30), w + 4)
        cv2.line(img, ip(a), ip(b), col, w)

    for n in ['Tail1', 'Tail2', 'TailEnd']:
        pass
    capsule(J['Hips'], J['Tail1'], 14, limb); capsule(J['Tail1'], J['Tail2'], 11, limb)
    capsule(J['Tail2'], J['TailEnd'], 8, limb)
    for s in 'LR':
        capsule(J['Hip_' + s], J['Knee_' + s], 16, limb); capsule(J['Knee_' + s], J['Ankle_' + s], 14, limb)
        cv2.ellipse(img, ip(J['Ankle_' + s]), (12, 7), 0, 0, 360, (40, 40, 40), -1)
    capsule(J['Hips'], J['Chest'], 46, body)
    for s in 'LR':
        capsule(J['Shoulder_' + s], J['Elbow_' + s], 13, limb); capsule(J['Elbow_' + s], J['Wrist_' + s], 11, limb)
        cv2.circle(img, ip(J['Wrist_' + s]), 9, (230, 230, 240), -1); cv2.circle(img, ip(J['Wrist_' + s]), 9, (30, 30, 30), 2)
    cv2.circle(img, ip(hc), L['head_r'], head, -1); cv2.circle(img, ip(hc), L['head_r'], (30, 30, 30), 3)
    for e in ('Eye_L', 'Eye_R'):
        cv2.circle(img, ip(J[e]), 9, (255, 255, 255), -1); cv2.circle(img, ip(J[e]), 9, (30, 30, 30), 2)
        cv2.circle(img, ip(J[e]), 4, (20, 20, 20), -1)
    cv2.ellipse(img, ip(J['Mouth']), (12, 6), p['head'] + 90, 0, 180, (30, 30, 30), 3)
    return img


def wobble_line(img, a, b, rnd, col, w=1):
    n = 8
    pts = []
    for i in range(n + 1):
        t = i / n
        x = a[0] + (b[0] - a[0]) * t + rnd.gauss(0, 1.6)
        y = a[1] + (b[1] - a[1]) * t + rnd.gauss(0, 1.6)
        pts.append((int(x), int(y)))
    cv2.polylines(img, [np.array(pts)], False, col, w, cv2.LINE_AA)


def sketch(J, hc, p, seed):
    rnd = random.Random(seed)
    img = np.full((520, 600, 3), 250, np.uint8)
    col = (70, 70, 70)

    def outline(a, b, w):
        ang = math.atan2(b[1] - a[1], b[0] - a[0]) + math.pi / 2
        dx, dy = math.cos(ang) * w / 2, math.sin(ang) * w / 2
        for _ in range(2):
            wobble_line(img, (a[0] + dx, a[1] + dy), (b[0] + dx, b[1] + dy), rnd, col)
            wobble_line(img, (a[0] - dx, a[1] - dy), (b[0] - dx, b[1] - dy), rnd, col)
        cv2.circle(img, ip(b), int(w / 2), col, 1, cv2.LINE_AA)

    for s in 'LR':
        outline(J['Hip_' + s], J['Knee_' + s], 16); outline(J['Knee_' + s], J['Ankle_' + s], 14)
        outline(J['Shoulder_' + s], J['Elbow_' + s], 13); outline(J['Elbow_' + s], J['Wrist_' + s], 11)
    outline(J['Hips'], J['Tail1'], 12); outline(J['Tail1'], J['Tail2'], 9); outline(J['Tail2'], J['TailEnd'], 6)
    outline(J['Hips'], J['Chest'], 46)
    for _ in range(2):
        r = L['head_r'] + rnd.uniform(-4, 4)
        pts = [(hc[0] + r * math.cos(t) + rnd.gauss(0, 1.5), hc[1] + r * math.sin(t) + rnd.gauss(0, 1.5))
               for t in np.linspace(0, 2 * math.pi, 40)]
        cv2.polylines(img, [np.array(pts, np.int32)], False, col, 1, cv2.LINE_AA)
    for e in ('Eye_L', 'Eye_R'):
        cv2.circle(img, ip(J[e]), 4, col, -1)
    cv2.ellipse(img, ip(J['Mouth']), (12, 6), p['head'] + 90, 0, 180, col, 1, cv2.LINE_AA)
    # a stray construction line, as boards have
    wobble_line(img, (J['Hips'][0] - 80, J['Hips'][1] + 90), (J['Hips'][0] + 120, J['Hips'][1] + 92), rnd, (150, 150, 150))
    return img


def main(out):
    os.makedirs(out, exist_ok=True)
    J, hc = joints(REST)
    cv2.imwrite(f'{out}/support.png', render(J, hc, REST))
    idx = {n: i for i, n in enumerate(NAMES)}
    json.dump({'names': NAMES, 'points': [list(J[n]) for n in NAMES],
               'skeleton': [[idx[a], idx[b]] for a, b in SKEL]}, open(f'{out}/support.json', 'w'))
    for k, p in enumerate(EXTRA, 1):
        J, hc = joints(p)
        cv2.imwrite(f'{out}/support{k}.png', render(J, hc, p))
        json.dump({'names': NAMES, 'points': [list(J[n]) for n in NAMES],
                   'skeleton': [[idx[a], idx[b]] for a, b in SKEL]}, open(f'{out}/support{k}.json', 'w'))
    truth = {}
    for i, (name, p) in enumerate(POSES.items()):
        J, hc = joints(p)
        cv2.imwrite(f'{out}/{name}_render.png', render(J, hc, p))
        truth[f'{name}_render'] = [list(J[n]) for n in NAMES]
        J, hc = joints(p, jitter=i)  # the artist's proportions are a little off
        cv2.imwrite(f'{out}/{name}_sketch.png', sketch(J, hc, p, i))
        truth[f'{name}_sketch'] = [list(J[n]) for n in NAMES]
    json.dump(truth, open(f'{out}/truth.json', 'w'))


if __name__ == '__main__':
    main(sys.argv[1])
