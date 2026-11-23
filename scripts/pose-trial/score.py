import json, sys, numpy as np
truth = json.load(open(sys.argv[1])); out = sys.argv[2]
names = None; per = {}
for k, t in sorted(truth.items()):
    r = json.load(open(f'{out}/{k}.json')); names = r['names']
    t = np.array(t); p = np.array(r['points'])
    size = (t.max(0) - t.min(0)).max()
    e = np.linalg.norm(p - t, axis=1) / size
    for n, v in zip(names, e): per.setdefault(n, []).append(v)
    print(f'{k:16s} mean {e.mean()*100:5.1f}%  within 5%: {(e<.05).mean()*100:3.0f}%  within 10%: {(e<.10).mean()*100:3.0f}%  worst {names[e.argmax()]} {e.max()*100:.0f}%')
print('per joint, mean error % of figure size:')
print('  '.join(f'{n} {np.mean(v)*100:.0f}' for n, v in per.items()))
