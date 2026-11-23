"""Headless PoseAnything: one support picture with its joints and skeleton, many queries.

python run_pa.py --repo PoseAnything --config ... --checkpoint ... \
    --support s.png --kp s.json --out outdir q1.png q2.png ...

s.json: {"names": [...], "points": [[x, y], ...] (support pixels), "skeleton": [[i, j], ...]}
Writes outdir/<query>.json (points in query pixels, a sureness each) and <query>_vis.png.
"""
import argparse, json, os, sys, time

import cv2
import numpy as np
import torch

S = 256  # set from the config in main()


def square(img, crop=True):
    """Crop to the ink (plus a margin), pad to a centred square with white, resize to S.
    Returns the picture and (scale, ox, oy): square = (pixel + o) * scale."""
    x0 = y0 = 0
    if crop:
        ink = np.argwhere(img.min(2) < 200)
        (y0, x0), (y1, x1) = ink.min(0), ink.max(0)
        m = int(0.08 * max(y1 - y0, x1 - x0))
        y0, x0 = max(0, y0 - m), max(0, x0 - m)
        y1, x1 = min(img.shape[0], y1 + m), min(img.shape[1], x1 + m)
        img = img[y0:y1, x0:x1]
    h, w = img.shape[:2]
    n = max(h, w)
    canvas = np.full((n, n, 3), 255, np.uint8)
    ox, oy = (n - w) // 2, (n - h) // 2
    canvas[oy:oy + h, ox:ox + w] = img
    return cv2.resize(canvas, (S, S), interpolation=cv2.INTER_AREA), (S / n, ox - x0, oy - y0)


def to_tensor(img):
    rgb = img[:, :, ::-1].astype(np.float32) / 255.0
    rgb = (rgb - np.array([0.485, 0.456, 0.406])) / np.array([0.229, 0.224, 0.225])
    return torch.from_numpy(rgb.transpose(2, 0, 1).copy()).float()[None]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--repo', required=True)
    ap.add_argument('--config', required=True)
    ap.add_argument('--checkpoint', required=True)
    ap.add_argument('--support', action='append', required=True)
    ap.add_argument('--kp', action='append', required=True)
    ap.add_argument('--out', required=True)
    ap.add_argument('--device', default='cpu')
    ap.add_argument('--impl', default='pa', choices=['pa', 'ec'], help='PoseAnything or EdgeCape')
    ap.add_argument('--box', action='append', default=[], help='x0,y0,x1,y1 crop for each query, in order')
    ap.add_argument('queries', nargs='+')
    a = ap.parse_args()

    sys.path.insert(0, os.path.abspath(a.repo))
    from mmcv import Config
    from mmcv.runner import load_checkpoint
    from mmpose.models import build_posenet
    if a.impl == 'pa':
        import models  # noqa: F401  registers PoseAnything's modules
        from models.datasets.pipelines import TopDownGenerateTargetFewShot
        sigma = 1
    else:
        sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), 'shims'))
        import EdgeCape  # noqa: F401
        from EdgeCape.datasets.pipelines import TopDownGenerateTargetFewShot
        sigma = 2

    cfg = Config.fromfile(a.config)
    global S
    S = cfg.model.get("encoder_config", {}).get("img_size", 256)
    if a.impl == 'ec':
        S = 252  # DINOv2 takes a multiple of its 14-pixel patch
    data_cfg = cfg.data_cfg
    data_cfg['image_size'] = np.array([S, S])
    data_cfg['joint_weights'] = None
    data_cfg['use_different_joint_weights'] = False
    S_imgs, S_t, S_w, S_kp3, S_kp = [], [], [], [], []
    for sup, kpf in zip(a.support, a.kp):
        spec = json.load(open(kpf))
        names, skeleton = spec['names'], [tuple(e) for e in spec['skeleton']]
        s_img, (sc, ox, oy) = square(cv2.imread(sup))
        kp = torch.tensor([[(x + ox) * sc, (y + oy) * sc] for x, y in spec['points']]).float()
        kp3 = torch.cat((kp, torch.zeros(len(kp), 1)), -1)
        w3 = torch.cat((torch.ones_like(kp), torch.zeros(len(kp), 1)), -1)
        t, w = TopDownGenerateTargetFewShot()._msra_generate_target(data_cfg, kp3, w3, sigma=sigma)
        S_imgs.append(s_img); S_t.append(torch.tensor(t).float()[None]); S_w.append(torch.tensor(w).float()[None])
        S_kp3.append(kp3); S_kp.append(kp)
    kp, kp3 = S_kp[0], S_kp3[0]

    t0 = time.time()
    model = build_posenet(cfg.model)
    load_checkpoint(model, a.checkpoint, map_location='cpu')
    model.eval().to(a.device)
    print(f'loaded in {time.time() - t0:.1f}s; {sum(p.numel() for p in model.parameters()) / 1e6:.1f} M weights')

    os.makedirs(a.out, exist_ok=True)
    dev = a.device
    for n, q in enumerate(a.queries):
        q_raw = cv2.imread(q)
        if n < len(a.box):
            x0, y0, x1, y1 = map(int, a.box[n].split(','))
            q_img, (qs, qx, qy) = square(q_raw[y0:y1, x0:x1], crop=False)
            qx, qy = qx - x0, qy - y0
        else:
            q_img, (qs, qx, qy) = square(q_raw)
        meta = {'sample_skeleton': [skeleton] * len(S_kp), 'query_skeleton': skeleton,
                'sample_joints_3d': S_kp3, 'query_joints_3d': kp3,
                'sample_center': [k.mean(0) for k in S_kp], 'query_center': kp.mean(0),
                'sample_scale': [k.max(0)[0] - k.min(0)[0] for k in S_kp], 'query_scale': kp.max(0)[0] - kp.min(0)[0],
                'sample_rotation': [0] * len(S_kp), 'query_rotation': 0, 'sample_bbox_score': [1] * len(S_kp), 'query_bbox_score': 1,
                'query_image_file': '', 'sample_image_file': [''] * len(S_kp)}
        t0 = time.time()
        with torch.no_grad():
            res = model.predict([to_tensor(i).to(dev) for i in S_imgs], [t.to(dev) for t in S_t], [w.to(dev) for w in S_w],
                                           to_tensor(q_img).to(dev), [meta])
            out, sim = res[0], res[2]
        dt = time.time() - t0
        pts = out[-1][0].cpu().numpy() * S  # [N, 2] in the square
        try:
            sure = sim[0].flatten(1).softmax(-1).max(-1)[0].cpu().numpy()
        except Exception:
            sure = np.ones(len(pts))
        back = [[float(x / qs - qx), float(y / qs - qy)] for x, y in pts[:len(names)]]
        base = os.path.splitext(os.path.basename(q))[0].replace(' ', '_') + (f'_{n}' if a.box else '')
        json.dump({'names': names, 'points': back, 'sure': sure[:len(names)].tolist(), 'seconds': dt},
                  open(os.path.join(a.out, base + '.json'), 'w'), indent=1)
        vis = q_raw.copy()
        r = max(2, int(max(vis.shape[:2]) / 150))
        for i, j in skeleton:
            cv2.line(vis, tuple(map(int, back[i])), tuple(map(int, back[j])), (0, 140, 255), max(1, r // 2))
        for x, y in back:
            cv2.circle(vis, (int(x), int(y)), r, (0, 0, 255), -1)
        cv2.imwrite(os.path.join(a.out, base + '_vis.png'), vis)
        print(f'{base}: {dt:.2f}s')


if __name__ == '__main__':
    main()
