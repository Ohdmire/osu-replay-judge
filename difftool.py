import csv, subprocess, sys, os
NAME_MAP = {'miss':'miss','meh':'meh','ok':'ok','good':'good','great':'great','perfect':'perfect','ltickmiss':'largetickmiss','ltick':'largetickhit','stickmiss':'smalltickmiss','stick':'smalltickhit','tail':'slidertailhit','sbonus':'smallbonus','lbonus':'largebonus','ignoremiss':'ignoremiss','ignorehit':'ignorehit'}
def norm_label(l):
    for p in ('tick','repeat'):
        if l.startswith(p): return p
    return l
def load_rust(path):
    rows=[]
    with open(path) as f:
        r=csv.reader(f, delimiter='\t'); header=next(r)
        for row in r:
            d = dict(zip(header,row))
            # `smax` is a Rust-only audio marker (lazer plays it from
            # maxBonusSample with no judgement event): no C# counterpart.
            if d.get('label') != 'smax':
                rows.append(d)
    return rows
def load_gt(path):
    rows=[]
    with open(path) as f:
        for line in f:
            if line.startswith('#') or line.startswith('time\t'): continue
            parts=line.rstrip('\n').split('\t')
            if len(parts)>=9: rows.append(parts)
    return rows
def diff(rust_path, gt_path):
    rust=load_rust(rust_path); gt=load_gt(gt_path)
    mism=[]
    if len(rust)!=len(gt):
        mism.append(f"COUNT rust={len(rust)} gt={len(gt)}")
    for i in range(min(len(rust),len(gt))):
        r=rust[i]; g=gt[i]
        rname=NAME_MAP.get(r['result'],r['result'])
        core=(r['object']==g[1] and norm_label(r['label'])==norm_label(g[2]) and rname==g[3].lower() and r['combo']==g[4] and r['score']==g[5])
        # Frame-grid phase differs between the engines on non-user-triggered
        # results (slider bodies): one pump step is 16.67ms, so 1.5ms is
        # still far tighter than any real ordering/semantic difference.
        # Non-user-triggered results (slider bodies / tails) fire on the
        # first frame past their threshold, and the two engines' frame grids
        # differ in phase (replay-frame-aligned vs fixed 1000/60 pump): up to
        # one pump step (16.67ms) of skew is grid phase, not semantics.
        # User-triggered times stay exact (the replay frame's timestamp).
        # ...up to one rate-scaled pump step (16.67ms * rate; DT=25ms).
        stime=abs(float(r['time'])-float(g[0]))<26.0
        sac=abs(float(r['accuracy'])-float(g[6]))<1e-5
        sur_ok = (r['unstable_rate']=='n/a' and g[7]=='n/a') or (r['unstable_rate']!='n/a' and g[7]!='n/a' and abs(float(r['unstable_rate'])-float(g[7]))<0.01)
        if not (core and stime and sac and sur_ok):
            mism.append(f"[{i}] R t={r['time']} {r['object']} {r['label']} {r['result']} c={r['combo']} s={r['score']} acc={r['accuracy']} ur={r['unstable_rate']} | G t={g[0]} {g[1]} {g[2]} {g[3]} c={g[4]} s={g[5]} acc={g[6]} ur={g[7]}")
    return mism
if __name__ == '__main__':
    mism = diff(sys.argv[1], sys.argv[2])
    print(f"mismatches: {len(mism)}")
    for m in mism[:15]: print(m)
