import json, sys, cv2, numpy as np
from rtmlib import Body
truth = json.load(open(sys.argv[1])); d = sys.argv[2]
names = json.load(open(f'{d}/support.json'))['names']
# COCO index -> our joint
M = {1: 'Eye_L', 2: 'Eye_R', 5: 'Shoulder_L', 6: 'Shoulder_R', 7: 'Elbow_L', 8: 'Elbow_R', 9: 'Wrist_L', 10: 'Wrist_R',
     11: 'Hip_L', 12: 'Hip_R', 13: 'Knee_L', 14: 'Knee_R', 15: 'Ankle_L', 16: 'Ankle_R'}
body = Body(mode='performance', backend='onnxruntime', device='cpu')
pa = sys.argv[3]
for k in sorted(truth):
    if k == 'self': continue
    t = np.array(truth[k]); size = (t.max(0) - t.min(0)).max()
    kps, sc = body(cv2.imread(f'{d}/{k}.png'))
    idx = [names.index(n) for n in M.values()]
    p_pa = np.array(json.load(open(f'{pa}/{k}.json'))['points'])[idx]
    e_pa = np.linalg.norm(p_pa - t[idx], axis=1) / size
    if len(kps) == 0:
        print(f'{k:16s} RTMPose: no figure found      PoseAnything-5 on same joints: mean {e_pa.mean()*100:4.1f}%'); continue
    p = kps[0][list(M)]
    # either side may be read mirrored; score the better of the two readings
    sw = p.copy(); sw[0::2], sw[1::2] = p[1::2], p[0::2]
    e = min((np.linalg.norm(q - t[idx], axis=1) / size for q in (p, sw)), key=np.mean)
    print(f'{k:16s} RTMPose mean {e.mean()*100:4.1f}% within 10%: {(e<.1).mean()*100:3.0f}%   PoseAnything-5 mean {e_pa.mean()*100:4.1f}% within 10%: {(e_pa<.1).mean()*100:3.0f}%')
