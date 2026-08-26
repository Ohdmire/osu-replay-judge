#!/usr/bin/env bash
# Batch verification: Rust engine timelines vs the DotNet (osu! lazer
# source) reference harness, one dotnet startup for the whole batch.
#
#   ./verify.sh [n]        verify everything (or the first n pairs)
#
# Pairs are auto-discovered from ~/下载/*.osr + the renderer testset,
# md5-matched against the osu-eyes beatmap cache. Classic (stable)
# mode is inferred from the replay version (< 30_000_000).
set -euo pipefail
cd "$(dirname "$0")"

OSU_REPO="${OSU_REPO:-$HOME/文档/GitHub/osu}"
WORK=verify_work
LIMIT="${1:-0}"

echo "== [1/4] building rust judge =="
cargo build --release --quiet

echo "== [2/4] assembling batch manifest =="
python3 - "$LIMIT" <<'PYEOF'
import struct, hashlib, glob, os, json, sys
limit = int(sys.argv[1])
def rs(f):
    if f.read(1)==b'\x0b':
        n=0;sh=0
        while True:
            x=struct.unpack('B',f.read(1))[0]; n|=(x&0x7f)<<sh; sh+=7
            if not(x&0x80): break
        return f.read(n).decode('utf-8','replace')
    f.seek(-1,1); return None
def header(path):
    f=open(path,'rb'); f.read(1); ver=struct.unpack('<i',f.read(4))[0]
    md5=rs(f); return ver, md5
bm={}
for p in glob.glob(os.path.expanduser('~/文档/GitHub/osu-eyes/cache/beatmaps/*.osu')):
    bm[hashlib.md5(open(p,'rb').read()).hexdigest()]=p
entries=[]
# The renderer testset carries a lazer-version replay.
for rep in glob.glob('../osu-replay-render/testset/*.osr'):
    ver,md5 = header(rep)
    map_ = bm.get(md5) or '../osu-replay-render/testset/THE ORAL CIGARETTES - ReI (Nevo) [Rain].osu'
    entries.append((os.path.basename(rep), map_, rep, ver))
# Player replays from the downloads folder.
for rep in sorted(glob.glob(os.path.expanduser('~/下载/*.osr'))):
    ver,md5 = header(rep)
    m = bm.get(md5)
    if not m: continue
    entries.append((os.path.basename(rep), m, rep, ver))
os.makedirs('verify_work', exist_ok=True)
seen=set(); batch=[]
for name, m, rep, ver in entries:
    key=(m,rep)
    if key in seen: continue
    seen.add(key)
    batch.append({
        'name': f"{ver//10000}_{name[:40]}".replace(' ','_'),
        'map': os.path.abspath(m),
        'replay': os.path.abspath(rep),
        'out': os.path.abspath(f'verify_work/gt_{len(batch):02d}.tsv'),
        'classic': ver < 30_000_000,
    })
    if limit and len(batch)>=limit: break
json.dump(batch, open('verify_work/manifest.json','w'), ensure_ascii=False, indent=1)
print(f'{len(batch)} pairs: {sum(b["classic"] for b in batch)} classic / {sum(not b["classic"] for b in batch)} lazer')
PYEOF

echo "== [3/4] rust timelines (parallel) =="
python3 - <<'PYEOF'
import json, subprocess, os
from concurrent.futures import ThreadPoolExecutor
batch = json.load(open('verify_work/manifest.json'))
def run(e):
    out = e['out'].replace('gt_','trs_')
    r = subprocess.run(['./target/release/osu_replay_judge',
                        e['map'], e['replay'], '--timeline', out],
                       capture_output=True, timeout=300)
    return e['name'], r.returncode, out
with ThreadPoolExecutor(max_workers=8) as ex:
    for name, rc, out in ex.map(run, batch):
        if rc != 0: print(f'  RUST FAIL {name}')
print('rust timelines done')
PYEOF

echo "== [4/4] dotnet reference (single startup, whole batch) =="
export PATH="$HOME/.dotnet:$PATH" DOTNET_CLI_TELEMETRY_OPTOUT=1
export OSU_JUDGE_BATCH="$PWD/verify_work/manifest.json"
dotnet test "$OSU_REPO/osu.Game.Rulesets.Osu.Tests/osu.Game.Rulesets.Osu.Tests.csproj" \
    -c Debug --nologo --filter 'FullyQualifiedName~TestSceneReplayJudgeHarness.RunBatch' \
    -- "NUnit.TestAdapter.DisableAppDomain=true" 2>&1 | tail -5

echo "== diff summary =="
python3 - <<'PYEOF'
import json, importlib.util
spec = importlib.util.spec_from_file_location('difftool', 'difftool.py')
dt = importlib.util.module_from_spec(spec); spec.loader.exec_module(dt)
batch = json.load(open('verify_work/manifest.json'))
total = 0
for e in batch:
    trs = e['out'].replace('gt_','trs_'); gt = e['out']
    try:
        mism = dt.diff(trs, gt)
        total += len(mism)
        print(f"{e['name'][:46]:48s} classic={int(e['classic'])} mismatches={len(mism)}")
        for m in mism[:4]: print('   ', m)
    except FileNotFoundError as err:
        print(f"{e['name'][:46]:48s} MISSING {err.filename}")
print(f"TOTAL mismatches: {total}")
PYEOF
